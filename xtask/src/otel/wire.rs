//! What an export holds, independent of its encoding, and its two
//! encodings: OTLP/JSON (the files `--out` writes, and
//! `OTEL_EXPORTER_OTLP_PROTOCOL=http/json`) and OTLP binary protobuf (what
//! is sent by default). Both follow opentelemetry-proto (Apache-2.0;
//! THIRD_PARTY.md): the JSON mapping (ids in hex, 64-bit integers as
//! strings, lowerCamelCase names) and the protobuf field numbers of
//! common/v1, resource/v1, trace/v1, logs/v1, metrics/v1 and the
//! collector's `Export*ServiceRequest` messages (ids as raw bytes, times
//! fixed64, enums varints, `as_int` sfixed64). Hand-written: no protoc,
//! no crate. Profiles (v1development) stay JSON (otel.rs `profiles_json`).

use super::{Attrs, Val, esc};
use std::fmt::Write as _;

/// One export's items, in absolute times (Unix ns).
#[derive(Clone, Debug, Default)]
pub struct Data {
    pub resource: Attrs,
    pub spans: Vec<Span>,
    pub logs: Vec<Log>,
    pub points: Vec<Point>,
}

#[derive(Clone, Debug)]
pub struct Span {
    pub trace_id: [u8; 16],
    pub span_id: u64,
    pub parent: Option<u64>,
    pub name: String,
    /// SpanKind (1 internal).
    pub kind: u8,
    pub start: u64,
    pub end: u64,
    pub attrs: Attrs,
    pub events: Vec<Event>,
    pub links: Vec<Link>,
    /// Status code (0 unset, 1 ok, 2 error) and message.
    pub status: u8,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub time: u64,
    pub name: String,
    pub attrs: Attrs,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub trace_id: [u8; 16],
    pub span_id: u64,
    pub attrs: Attrs,
}

#[derive(Clone, Debug)]
pub struct Log {
    pub time: u64,
    pub severity: u8,
    pub text: String,
    pub body: String,
    pub attrs: Attrs,
    /// The span the record belongs to (trace id, span id).
    pub context: Option<([u8; 16], u64)>,
}

/// One data point and the metric it belongs to; a request groups
/// consecutive points of the same metric into one `Metric`.
#[derive(Clone, Debug)]
pub struct Point {
    pub name: String,
    pub unit: String,
    /// A monotonic cumulative sum (its start time `start`), else a gauge.
    pub sum: bool,
    pub start: u64,
    pub time: u64,
    pub value: i64,
    pub attrs: Attrs,
}

impl Point {
    fn same_metric(&self, o: &Point) -> bool {
        self.name == o.name && self.unit == o.unit && self.sum == o.sum
    }
}

/// The instrumentation scope of every request.
const SCOPE_NAME: &str = "gala-xtask-otel";
const SCOPE_VERSION: &str = "1";

pub fn hex_id(id: &[u8]) -> String {
    id.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ---- OTLP/JSON ----

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

pub fn attrs_json(out: &mut String, attrs: &[(String, Val)]) {
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

/// The `resource` object of every request.
pub fn resource_json(attrs: &Attrs) -> String {
    let mut o = String::from("{\"attributes\":");
    attrs_json(&mut o, attrs);
    o.push('}');
    o
}

pub fn scope_json() -> String {
    format!("{{\"name\":\"{SCOPE_NAME}\",\"version\":\"{SCOPE_VERSION}\"}}")
}

fn span_json(o: &mut String, sp: &Span) {
    let _ =
        write!(o, "{{\"traceId\":\"{}\",\"spanId\":\"{:016x}\"", hex_id(&sp.trace_id), sp.span_id);
    if let Some(p) = sp.parent {
        let _ = write!(o, ",\"parentSpanId\":\"{p:016x}\"");
    }
    o.push_str(",\"name\":");
    esc(o, &sp.name);
    let _ = write!(
        o,
        ",\"kind\":{},\"startTimeUnixNano\":\"{}\",\"endTimeUnixNano\":\"{}\",\"attributes\":",
        sp.kind, sp.start, sp.end
    );
    attrs_json(o, &sp.attrs);
    o.push_str(",\"events\":[");
    for (i, e) in sp.events.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(o, "{{\"timeUnixNano\":\"{}\",\"name\":", e.time);
        esc(o, &e.name);
        o.push_str(",\"attributes\":");
        attrs_json(o, &e.attrs);
        o.push('}');
    }
    o.push_str("],\"links\":[");
    for (i, l) in sp.links.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(
            o,
            "{{\"traceId\":\"{}\",\"spanId\":\"{:016x}\",\"attributes\":",
            hex_id(&l.trace_id),
            l.span_id
        );
        attrs_json(o, &l.attrs);
        o.push('}');
    }
    let _ = write!(o, "],\"status\":{{\"code\":{}", sp.status);
    if !sp.message.is_empty() {
        o.push_str(",\"message\":");
        esc(o, &sp.message);
    }
    o.push_str("}}");
}

/// An `ExportTraceServiceRequest` of `spans` (`resource`: `resource_json`).
pub fn traces_json(resource: &str, spans: &[Span]) -> String {
    let mut o = format!(
        "{{\"resourceSpans\":[{{\"resource\":{resource},\"scopeSpans\":[{{\"scope\":{},\"spans\":[",
        scope_json()
    );
    for (i, sp) in spans.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        span_json(&mut o, sp);
    }
    o.push_str("]}]}]}");
    o
}

/// An `ExportLogsServiceRequest` of `logs`.
pub fn logs_json(resource: &str, logs: &[Log]) -> String {
    let mut o = format!(
        "{{\"resourceLogs\":[{{\"resource\":{resource},\"scopeLogs\":[{{\"scope\":{},\"logRecords\":[",
        scope_json()
    );
    for (i, l) in logs.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(
            o,
            "{{\"timeUnixNano\":\"{}\",\"observedTimeUnixNano\":\"{}\",\"severityNumber\":{}",
            l.time, l.time, l.severity
        );
        if !l.text.is_empty() {
            o.push_str(",\"severityText\":");
            esc(&mut o, &l.text);
        }
        o.push_str(",\"body\":{\"stringValue\":");
        esc(&mut o, &l.body);
        o.push_str("},\"attributes\":");
        attrs_json(&mut o, &l.attrs);
        if let Some((trace, span)) = l.context {
            let _ = write!(o, ",\"traceId\":\"{}\",\"spanId\":\"{span:016x}\"", hex_id(&trace));
        }
        o.push('}');
    }
    o.push_str("]}]}]}");
    o
}

/// An `ExportMetricsServiceRequest` of `points` (consecutive points of
/// one metric make one `Metric`).
pub fn metrics_json(resource: &str, points: &[Point]) -> String {
    let mut m = format!(
        "{{\"resourceMetrics\":[{{\"resource\":{resource},\"scopeMetrics\":[{{\"scope\":{},\"metrics\":[",
        scope_json()
    );
    for (i, group) in points.chunk_by(Point::same_metric).enumerate() {
        let metric = &group[0];
        if i > 0 {
            m.push(',');
        }
        m.push_str("{\"name\":");
        esc(&mut m, &metric.name);
        m.push_str(",\"unit\":");
        esc(&mut m, &metric.unit);
        if metric.sum {
            m.push_str(
                ",\"sum\":{\"aggregationTemporality\":2,\"isMonotonic\":true,\"dataPoints\":[",
            );
        } else {
            m.push_str(",\"gauge\":{\"dataPoints\":[");
        }
        for (j, p) in group.iter().enumerate() {
            if j > 0 {
                m.push(',');
            }
            if p.sum {
                let _ = write!(m, "{{\"startTimeUnixNano\":\"{}\",", p.start);
            } else {
                m.push('{');
            }
            let _ = write!(
                m,
                "\"timeUnixNano\":\"{}\",\"asInt\":\"{}\",\"attributes\":",
                p.time, p.value
            );
            attrs_json(&mut m, &p.attrs);
            m.push('}');
        }
        m.push_str("]}}");
    }
    m.push_str("]}]}]}");
    m
}

// ---- OTLP protobuf ----

/// Protobuf wire types.
const VARINT: u32 = 0;
const I64: u32 = 1;
const LEN: u32 = 2;

fn varint(o: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        o.push(v as u8 | 0x80);
        v >>= 7;
    }
    o.push(v as u8);
}

fn key(o: &mut Vec<u8>, field: u32, wire: u32) {
    varint(o, u64::from(field << 3 | wire));
}

/// A length-delimited field (string, bytes), always written.
fn bytes(o: &mut Vec<u8>, field: u32, b: &[u8]) {
    key(o, field, LEN);
    varint(o, b.len() as u64);
    o.extend_from_slice(b);
}

/// A proto3 string or bytes field: left out when empty (its default).
fn opt_bytes(o: &mut Vec<u8>, field: u32, b: &[u8]) {
    if !b.is_empty() {
        bytes(o, field, b);
    }
}

/// A proto3 varint field (enum, uint, bool): left out when 0.
fn opt_varint(o: &mut Vec<u8>, field: u32, v: u64) {
    if v != 0 {
        key(o, field, VARINT);
        varint(o, v);
    }
}

/// A fixed64 field: left out when 0.
fn opt_fixed64(o: &mut Vec<u8>, field: u32, v: u64) {
    if v != 0 {
        key(o, field, I64);
        o.extend_from_slice(&v.to_le_bytes());
    }
}

/// An embedded message, always written (present even when empty).
fn msg(o: &mut Vec<u8>, field: u32, body: impl FnOnce(&mut Vec<u8>)) {
    let mut m = Vec::new();
    body(&mut m);
    bytes(o, field, &m);
}

/// AnyValue: a oneof, so its member is written even when it holds the
/// default (0, false, "").
fn any_pb(o: &mut Vec<u8>, v: &Val) {
    match v {
        Val::Str(t) => bytes(o, 1, t.as_bytes()),
        Val::Bool(b) => {
            key(o, 2, VARINT);
            varint(o, u64::from(*b));
        }
        // int64: two's complement in a 10-byte varint when negative.
        Val::Int(i) => {
            key(o, 3, VARINT);
            varint(o, *i as u64);
        }
        Val::Arr(a) => msg(o, 5, |m| {
            for v in a {
                msg(m, 1, |n| any_pb(n, v));
            }
        }),
    }
}

/// Repeated KeyValue at `field`.
fn attrs_pb(o: &mut Vec<u8>, field: u32, attrs: &[(String, Val)]) {
    for (k, v) in attrs {
        msg(o, field, |m| {
            opt_bytes(m, 1, k.as_bytes());
            msg(m, 2, |n| any_pb(n, v));
        });
    }
}

/// A `Resource` message's bytes (`attributes` = 1), encoded once per
/// exporter.
pub fn resource_pb(attrs: &Attrs) -> Vec<u8> {
    let mut o = Vec::new();
    attrs_pb(&mut o, 1, attrs);
    o
}

/// The `Export*ServiceRequest` frame shared by the three signals:
/// `resource_*` = 1 { resource = 1, scope_* = 2 { scope = 1, items = 2 } }.
fn request_pb(resource: &[u8], items: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut o = Vec::new();
    msg(&mut o, 1, |r| {
        bytes(r, 1, resource);
        msg(r, 2, |s| {
            msg(s, 1, |scope| {
                bytes(scope, 1, SCOPE_NAME.as_bytes());
                bytes(scope, 2, SCOPE_VERSION.as_bytes());
            });
            items(s);
        });
    });
    o
}

fn span_pb(o: &mut Vec<u8>, sp: &Span) {
    bytes(o, 1, &sp.trace_id);
    bytes(o, 2, &sp.span_id.to_be_bytes());
    if let Some(p) = sp.parent {
        bytes(o, 4, &p.to_be_bytes());
    }
    opt_bytes(o, 5, sp.name.as_bytes());
    opt_varint(o, 6, u64::from(sp.kind));
    opt_fixed64(o, 7, sp.start);
    opt_fixed64(o, 8, sp.end);
    attrs_pb(o, 9, &sp.attrs);
    for e in &sp.events {
        msg(o, 11, |m| {
            opt_fixed64(m, 1, e.time);
            opt_bytes(m, 2, e.name.as_bytes());
            attrs_pb(m, 3, &e.attrs);
        });
    }
    for l in &sp.links {
        msg(o, 13, |m| {
            bytes(m, 1, &l.trace_id);
            bytes(m, 2, &l.span_id.to_be_bytes());
            attrs_pb(m, 4, &l.attrs);
        });
    }
    msg(o, 15, |m| {
        opt_bytes(m, 2, sp.message.as_bytes());
        opt_varint(m, 3, u64::from(sp.status));
    });
}

/// An `ExportTraceServiceRequest` of `spans` (`resource`: `resource_pb`).
pub fn traces_pb(resource: &[u8], spans: &[Span]) -> Vec<u8> {
    request_pb(resource, |s| {
        for sp in spans {
            msg(s, 2, |m| span_pb(m, sp));
        }
    })
}

/// An `ExportLogsServiceRequest` of `logs`.
pub fn logs_pb(resource: &[u8], logs: &[Log]) -> Vec<u8> {
    request_pb(resource, |s| {
        for l in logs {
            msg(s, 2, |m| {
                opt_fixed64(m, 1, l.time);
                opt_varint(m, 2, u64::from(l.severity));
                opt_bytes(m, 3, l.text.as_bytes());
                msg(m, 5, |b| bytes(b, 1, l.body.as_bytes()));
                attrs_pb(m, 6, &l.attrs);
                if let Some((trace, span)) = l.context {
                    bytes(m, 9, &trace);
                    bytes(m, 10, &span.to_be_bytes());
                }
                opt_fixed64(m, 11, l.time);
            });
        }
    })
}

/// An `ExportMetricsServiceRequest` of `points`.
pub fn metrics_pb(resource: &[u8], points: &[Point]) -> Vec<u8> {
    request_pb(resource, |s| {
        for group in points.chunk_by(Point::same_metric) {
            let metric = &group[0];
            msg(s, 2, |m| {
                opt_bytes(m, 1, metric.name.as_bytes());
                opt_bytes(m, 3, metric.unit.as_bytes());
                let points = |d: &mut Vec<u8>| {
                    for p in group {
                        msg(d, 1, |n| {
                            attrs_pb(n, 7, &p.attrs);
                            if p.sum {
                                opt_fixed64(n, 2, p.start);
                            }
                            opt_fixed64(n, 3, p.time);
                            // as_int: sfixed64 in the `value` oneof,
                            // written even when 0.
                            key(n, 6, I64);
                            n.extend_from_slice(&p.value.to_le_bytes());
                        });
                    }
                };
                if metric.sum {
                    // Sum: data_points 1, aggregation_temporality 2
                    // (cumulative), is_monotonic 3.
                    msg(m, 7, |d| {
                        points(d);
                        opt_varint(d, 2, 2);
                        opt_varint(d, 3, 1);
                    });
                } else {
                    msg(m, 5, points);
                }
            });
        }
    })
}
