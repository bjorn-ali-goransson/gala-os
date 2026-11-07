//! Sending an export to an OTLP/HTTP endpoint (docs/OBSERVABILITY.md
//! "OpenTelemetry export"): binary protobuf by default
//! (`OTEL_EXPORTER_OTLP_PROTOCOL=http/json` for JSON), gzip unless
//! `OTEL_EXPORTER_OTLP_COMPRESSION=none`, at most [`MAX_BATCH`] items a
//! request. Each signal has a [`Batcher`]: a bounded queue whose drops are
//! counted, giving a batch when full or [`INTERVAL`] after the last one.
//! The export after a boot sends a batch whenever one is full and then
//! flushes the rest; a live exporter (ROADMAP OT5) pushes as items happen
//! and calls [`Exporter::tick`]. A failed request is retried with
//! exponential backoff, honouring `Retry-After` on 429 and 503.

use super::wire::{self, Data, Log, Point, Span};
use super::{Attrs, Export};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// Items (spans, log records, data points) per request at most.
pub const MAX_BATCH: usize = 512;
/// Items a signal's queue holds before it drops new ones.
pub const QUEUE: usize = 8 * MAX_BATCH;
/// How long items wait for a batch to fill before going anyway.
pub const INTERVAL: Duration = Duration::from_secs(1);

/// A bounded queue of one signal's items, taken in batches.
pub struct Batcher<T> {
    queue: VecDeque<T>,
    capacity: usize,
    max_batch: usize,
    interval: Duration,
    last: Instant,
    /// Items refused because the queue was full.
    pub dropped: u64,
}

impl<T> Batcher<T> {
    pub fn new(capacity: usize, max_batch: usize, interval: Duration, now: Instant) -> Self {
        Batcher { queue: VecDeque::new(), capacity, max_batch, interval, last: now, dropped: 0 }
    }
    /// Queues `item`; false (and counted in `dropped`) when the queue is
    /// full.
    pub fn push(&mut self, item: T) -> bool {
        if self.queue.len() >= self.capacity {
            self.dropped += 1;
            return false;
        }
        self.queue.push_back(item);
        true
    }
    pub fn len(&self) -> usize {
        self.queue.len()
    }
    /// Whether a batch should go at `now`: a full one is waiting, or items
    /// have waited `interval` since the last batch.
    pub fn due(&self, now: Instant) -> bool {
        self.queue.len() >= self.max_batch
            || (!self.queue.is_empty() && now.duration_since(self.last) >= self.interval)
    }
    /// Marks a batch as done at `now`: the next is due `interval` later.
    pub fn done(&mut self, now: Instant) {
        self.last = now;
    }
    /// The oldest items, at most a batch of them.
    pub fn take(&mut self, now: Instant) -> Vec<T> {
        self.last = now;
        let n = self.queue.len().min(self.max_batch);
        self.queue.drain(..n).collect()
    }
}

/// The wire format (`OTEL_EXPORTER_OTLP_PROTOCOL`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Protocol {
    Protobuf,
    Json,
}

impl Protocol {
    /// `http/protobuf` (the default) or `http/json`.
    pub fn parse(value: Option<&str>) -> Result<Protocol, String> {
        match value.map(str::trim) {
            None | Some("") | Some("http/protobuf") => Ok(Protocol::Protobuf),
            Some("http/json") => Ok(Protocol::Json),
            Some(other) => Err(format!(
                "OTEL_EXPORTER_OTLP_PROTOCOL={other}: only http/protobuf (the default) and http/json are supported"
            )),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Protocol::Protobuf => "http/protobuf",
            Protocol::Json => "http/json",
        }
    }
    fn content_type(self) -> &'static str {
        match self {
            Protocol::Protobuf => "application/x-protobuf",
            Protocol::Json => "application/json",
        }
    }
}

/// `OTEL_EXPORTER_OTLP_COMPRESSION`: gzip (the default here) or none.
pub fn parse_compression(value: Option<&str>) -> Result<bool, String> {
    match value.map(str::trim) {
        None | Some("") | Some("gzip") => Ok(true),
        Some("none") => Ok(false),
        Some(other) => Err(format!(
            "OTEL_EXPORTER_OTLP_COMPRESSION={other}: only gzip (the default) and none are supported"
        )),
    }
}

/// How a failed request is retried: `attempts` in all, waiting `first`,
/// then twice as long each time up to `max`; a `Retry-After` (seconds) on
/// 429 or 503 replaces the wait, up to `retry_after_max`.
#[derive(Clone, Copy, Debug)]
pub struct Retry {
    pub attempts: u32,
    pub first: Duration,
    pub max: Duration,
    pub retry_after_max: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Retry {
            attempts: 5,
            first: Duration::from_secs(1),
            max: Duration::from_secs(16),
            retry_after_max: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Signal {
    Traces,
    Logs,
    Metrics,
    Profiles,
}

const SIGNALS: [Signal; 4] = [Signal::Traces, Signal::Logs, Signal::Metrics, Signal::Profiles];

impl Signal {
    fn path(self) -> &'static str {
        match self {
            Signal::Traces => "v1/traces",
            Signal::Logs => "v1/logs",
            Signal::Metrics => "v1/metrics",
            // Profiles are still a development signal: their own path.
            Signal::Profiles => "v1development/profiles",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Signal::Traces => "traces",
            Signal::Logs => "logs",
            Signal::Metrics => "metrics",
            Signal::Profiles => "profiles",
        }
    }
    fn items(self) -> &'static str {
        match self {
            Signal::Traces => "spans",
            Signal::Logs => "log records",
            Signal::Metrics => "points",
            Signal::Profiles => "profile",
        }
    }
}

/// What one signal's sending did.
#[derive(Clone, Debug, Default)]
pub struct SignalStats {
    /// Requests accepted (2xx).
    pub batches: u64,
    pub items: u64,
    /// Bytes on the wire (after gzip) and before compression.
    pub bytes: u64,
    pub raw: u64,
    /// Items of batches given up after their retries.
    pub failed: u64,
    /// Items never sent because the export stopped at a failed batch.
    pub unsent: u64,
    /// Items the endpoint rejected in an accepted request (OTLP partial
    /// success).
    pub rejected: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub signals: [SignalStats; 4],
    pub retries: u64,
    /// Why the latest retry happened (`HTTP 503, Retry-After 1 s, on
    /// /v1/traces`).
    pub last_retry: Option<String>,
    /// The endpoint's partial-success messages.
    pub rejections: Vec<String>,
}

impl Stats {
    pub fn of(&self, s: Signal) -> &SignalStats {
        &self.signals[s as usize]
    }
}

/// Sends batches of spans, log records and data points to one endpoint.
pub struct Exporter {
    host: String,
    prefix: String,
    pub protocol: Protocol,
    pub gzip: bool,
    pub retry: Retry,
    res_json: String,
    res_pb: Vec<u8>,
    spans: Batcher<Span>,
    logs: Batcher<Log>,
    points: Batcher<Point>,
    pub stats: Stats,
}

impl Exporter {
    /// An exporter to `endpoint` (`http://host:4318[/prefix]`, as in
    /// `OTEL_EXPORTER_OTLP_ENDPOINT`; plain HTTP only).
    pub fn new(
        endpoint: &str,
        protocol: Protocol,
        gzip: bool,
        resource: &Attrs,
    ) -> Result<Exporter, String> {
        let rest = endpoint.strip_prefix("http://").ok_or_else(|| {
            format!("{endpoint}: only http:// endpoints (OTLP/HTTP) are supported")
        })?;
        let (host, prefix) = match rest.split_once('/') {
            Some((h, p)) => (h, format!("/{}", p.trim_end_matches('/'))),
            None => (rest, String::new()),
        };
        let host = if host.contains(':') { host.to_string() } else { format!("{host}:80") };
        let now = Instant::now();
        Ok(Exporter {
            host,
            prefix: prefix.trim_end_matches('/').to_string(),
            protocol,
            gzip,
            retry: Retry::default(),
            res_json: wire::resource_json(resource),
            res_pb: wire::resource_pb(resource),
            spans: Batcher::new(QUEUE, MAX_BATCH, INTERVAL, now),
            logs: Batcher::new(QUEUE, MAX_BATCH, INTERVAL, now),
            points: Batcher::new(QUEUE, MAX_BATCH, INTERVAL, now),
            stats: Stats::default(),
        })
    }

    /// Queues a span; false when the queue is full (counted).
    pub fn push_span(&mut self, s: Span) -> bool {
        self.spans.push(s)
    }
    pub fn push_log(&mut self, l: Log) -> bool {
        self.logs.push(l)
    }
    pub fn push_point(&mut self, p: Point) -> bool {
        self.points.push(p)
    }

    /// Sends every batch due at `now` (a live exporter calls this about
    /// once a second).
    pub fn tick(&mut self, now: Instant) -> Result<(), String> {
        while self.spans.due(now) {
            self.send(Signal::Traces, now)?;
        }
        while self.logs.due(now) {
            self.send(Signal::Logs, now)?;
        }
        while self.points.due(now) {
            self.send(Signal::Metrics, now)?;
        }
        Ok(())
    }

    /// Sends everything queued.
    pub fn flush(&mut self) -> Result<(), String> {
        for signal in [Signal::Traces, Signal::Logs, Signal::Metrics] {
            while self.queued(signal) > 0 {
                self.send(signal, Instant::now())?;
            }
        }
        Ok(())
    }

    fn queued(&self, signal: Signal) -> usize {
        match signal {
            Signal::Traces => self.spans.len(),
            Signal::Logs => self.logs.len(),
            Signal::Metrics => self.points.len(),
            Signal::Profiles => 0,
        }
    }

    /// Sends all of `data`: a batch whenever one is full, then the rest.
    /// Stops at the first batch that fails its retries; what was not sent
    /// is counted (`unsent`).
    pub fn export(&mut self, data: &Data) -> Result<(), String> {
        let result = self.export_inner(data);
        if result.is_err() {
            for signal in [Signal::Traces, Signal::Logs, Signal::Metrics] {
                let total = match signal {
                    Signal::Traces => data.spans.len(),
                    Signal::Logs => data.logs.len(),
                    _ => data.points.len(),
                } as u64;
                let s = &mut self.stats.signals[signal as usize];
                s.unsent = total.saturating_sub(s.items + s.failed);
            }
        }
        result
    }

    fn export_inner(&mut self, data: &Data) -> Result<(), String> {
        // As a live exporter would: queue, send what is due (a full batch,
        // or a second's worth), then flush the rest. A queue never fills
        // here, as a full batch goes before the next item is queued.
        for s in &data.spans {
            self.push_span(s.clone());
            self.tick(Instant::now())?;
        }
        for l in &data.logs {
            self.push_log(l.clone());
            self.tick(Instant::now())?;
        }
        for p in &data.points {
            self.push_point(p.clone());
            self.tick(Instant::now())?;
        }
        self.flush()
    }

    /// Sends one OTLP/JSON profiles request (profiles stay JSON).
    pub fn send_profiles(&mut self, json: &str) -> Result<(), String> {
        self.deliver(Signal::Profiles, 1, json.as_bytes().to_vec(), "application/json")
    }

    /// Takes a batch of `signal` and sends it.
    fn send(&mut self, signal: Signal, now: Instant) -> Result<(), String> {
        let pb = self.protocol == Protocol::Protobuf;
        let (n, body) = match signal {
            Signal::Traces => {
                let b = self.spans.take(now);
                let body = if pb {
                    wire::traces_pb(&self.res_pb, &b)
                } else {
                    wire::traces_json(&self.res_json, &b).into_bytes()
                };
                (b.len(), body)
            }
            Signal::Logs => {
                let b = self.logs.take(now);
                let body = if pb {
                    wire::logs_pb(&self.res_pb, &b)
                } else {
                    wire::logs_json(&self.res_json, &b).into_bytes()
                };
                (b.len(), body)
            }
            Signal::Metrics => {
                let b = self.points.take(now);
                let body = if pb {
                    wire::metrics_pb(&self.res_pb, &b)
                } else {
                    wire::metrics_json(&self.res_json, &b).into_bytes()
                };
                (b.len(), body)
            }
            Signal::Profiles => return Ok(()),
        };
        if n == 0 {
            return Ok(());
        }
        let result = self.deliver(signal, n, body, self.protocol.content_type());
        // The interval counts from when this batch was done (its retries
        // included), not from when it was taken.
        let done = Instant::now();
        match signal {
            Signal::Traces => self.spans.done(done),
            Signal::Logs => self.logs.done(done),
            _ => self.points.done(done),
        }
        result
    }

    /// POSTs one request of `n` items, retrying as `retry` says.
    fn deliver(
        &mut self,
        signal: Signal,
        n: usize,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<(), String> {
        let raw = body.len() as u64;
        let body = if self.gzip { gzip(&body)? } else { body };
        let path = format!("{}/{}", self.prefix, signal.path());
        let mut wait = self.retry.first;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let r = http_post(&self.host, &path, content_type, self.gzip, &body);
            let (error, retry_after) = match r {
                Ok(resp) if (200..300).contains(&resp.status) => {
                    let s = &mut self.stats.signals[signal as usize];
                    s.batches += 1;
                    s.items += n as u64;
                    s.bytes += body.len() as u64;
                    s.raw += raw;
                    if let Some((rejected, message)) = partial_success(content_type, &resp.body) {
                        s.rejected += rejected;
                        self.stats.rejections.push(format!(
                            "{path}: {rejected} {} rejected: {message}",
                            signal.items()
                        ));
                    }
                    return Ok(());
                }
                Ok(resp) => {
                    let text = String::from_utf8_lossy(&resp.body);
                    let text: String = text.trim().chars().take(300).collect();
                    let error = format!("HTTP {}: {text}", resp.status);
                    // OTLP/HTTP: 429, 502, 503 and 504 are retryable,
                    // other statuses are not.
                    if !matches!(resp.status, 429 | 502 | 503 | 504) {
                        self.stats.signals[signal as usize].failed += n as u64;
                        return Err(format!("POST {path} to {}: {error}", self.host));
                    }
                    let after = match resp.status {
                        429 | 503 => resp.retry_after,
                        _ => None,
                    };
                    (error, after)
                }
                // Connection failures are retried too.
                Err(e) => (e, None),
            };
            if attempt >= self.retry.attempts {
                self.stats.signals[signal as usize].failed += n as u64;
                return Err(format!(
                    "POST {path} to {}: {error} (gave up after {attempt} attempts)",
                    self.host
                ));
            }
            let delay = match retry_after {
                Some(secs) => Duration::from_secs(secs).min(self.retry.retry_after_max),
                None => wait,
            };
            wait = (wait * 2).min(self.retry.max);
            self.stats.retries += 1;
            self.stats.last_retry = Some(match retry_after {
                Some(secs) => format!("{error}, Retry-After {secs} s, on {path}"),
                None => format!("{error}, on {path}, waited {} ms", delay.as_millis()),
            });
            std::thread::sleep(delay);
        }
    }

    /// One line for the `otel:` report: per signal the requests, items
    /// and bytes; retries, drops, failures and rejections.
    pub fn summary(&self) -> String {
        let mut o = format!("{}{}", self.protocol.name(), if self.gzip { ", gzip" } else { "" });
        let mut dropped = 0;
        for signal in SIGNALS {
            let s = self.stats.of(signal);
            let d = match signal {
                Signal::Traces => self.spans.dropped,
                Signal::Logs => self.logs.dropped,
                Signal::Metrics => self.points.dropped,
                Signal::Profiles => 0,
            };
            dropped += d;
            if s.batches + s.failed + s.unsent + d == 0 {
                continue;
            }
            let _ = write!(
                o,
                "; {} {} request{}, {} {}, {} KiB",
                signal.name(),
                s.batches,
                if s.batches == 1 { "" } else { "s" },
                s.items,
                signal.items(),
                s.bytes.div_ceil(1024)
            );
            if self.gzip {
                let _ = write!(o, " ({} KiB before gzip)", s.raw.div_ceil(1024));
            }
            if signal == Signal::Profiles {
                o.push_str(" (JSON)");
            }
            if d > 0 {
                let _ = write!(o, ", {d} DROPPED (queue full)");
            }
            if s.failed > 0 {
                let _ = write!(o, ", {} FAILED", s.failed);
            }
            if s.unsent > 0 {
                let _ = write!(o, ", {} NOT SENT", s.unsent);
            }
            if s.rejected > 0 {
                let _ = write!(o, ", {} REJECTED", s.rejected);
            }
        }
        let _ = write!(
            o,
            "; {} retr{}",
            self.stats.retries,
            if self.stats.retries == 1 { "y" } else { "ies" }
        );
        if let Some(why) = &self.stats.last_retry {
            let _ = write!(o, " (last: {why})");
        }
        let _ = write!(o, "; {dropped} dropped");
        for r in &self.stats.rejections {
            let _ = write!(o, "; {r}");
        }
        o
    }
}

/// Sends `export` to `endpoint` as the environment says
/// (`OTEL_EXPORTER_OTLP_PROTOCOL`, `OTEL_EXPORTER_OTLP_COMPRESSION`):
/// traces, logs and metrics in batches, then the profile (JSON). Returns
/// the summary, or the error with the summary.
pub fn post(endpoint: &str, export: &Export) -> Result<String, String> {
    let protocol = Protocol::parse(std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").ok().as_deref())?;
    let gzip = parse_compression(std::env::var("OTEL_EXPORTER_OTLP_COMPRESSION").ok().as_deref())?;
    let mut x = Exporter::new(endpoint, protocol, gzip, &export.data.resource)?;
    post_with(&mut x, export)
}

pub fn post_with(x: &mut Exporter, export: &Export) -> Result<String, String> {
    let mut result = x.export(&export.data);
    if result.is_ok() && !export.profiles.is_empty() {
        result = x.send_profiles(&export.profiles);
    }
    match result {
        Ok(()) => Ok(x.summary()),
        Err(e) => Err(format!("{e}; {}", x.summary())),
    }
}

fn gzip(body: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;
    let mut z = GzEncoder::new(Vec::new(), Compression::default());
    z.write_all(body).and_then(|()| z.finish()).map_err(|e| format!("gzip: {e}"))
}

/// The rejected item count and message of an OTLP partial success
/// (`Export*ServiceResponse.partial_success` = 1 { rejected_* = 1,
/// error_message = 2 }), when the response has a non-empty one.
pub fn partial_success(content_type: &str, body: &[u8]) -> Option<(u64, String)> {
    if body.is_empty() {
        return None;
    }
    let (rejected, message) = if content_type == "application/x-protobuf" {
        let inner = pb_field(body, 1)?;
        let rejected = pb_varint_field(inner, 1).unwrap_or(0);
        let message = pb_field(inner, 2).map(|m| String::from_utf8_lossy(m).into_owned());
        (rejected, message.unwrap_or_default())
    } else {
        let json = super::parse_json(std::str::from_utf8(body).ok()?).ok()?;
        let p = json.get("partialSuccess")?;
        let super::Json::Obj(fields) = p else { return None };
        let rejected = fields
            .iter()
            .find(|(k, _)| k.starts_with("rejected"))
            .and_then(|(_, v)| match v {
                super::Json::Num(n) | super::Json::Str(n) => n.parse().ok(),
                _ => None,
            })
            .unwrap_or(0);
        (rejected, p.str("errorMessage").unwrap_or_default().to_string())
    };
    (rejected > 0 || !message.is_empty()).then_some((rejected, message))
}

/// Reads a protobuf varint at `*i`.
fn pb_varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*i)?;
        *i += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some(v);
        }
    }
    None
}

/// Walks a message's fields: (field, wire type, varint value or
/// length-delimited bytes).
fn pb_fields(b: &[u8]) -> impl Iterator<Item = (u64, u64, u64, &[u8])> {
    let mut i = 0;
    std::iter::from_fn(move || {
        if i >= b.len() {
            return None;
        }
        let k = pb_varint(b, &mut i)?;
        match k & 7 {
            0 => Some((k >> 3, 0, pb_varint(b, &mut i)?, &b[0..0])),
            1 => {
                let v = u64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?);
                i += 8;
                Some((k >> 3, 1, v, &b[0..0]))
            }
            2 => {
                let len = pb_varint(b, &mut i)? as usize;
                let d = b.get(i..i + len)?;
                i += len;
                Some((k >> 3, 2, 0, d))
            }
            5 => {
                let v = u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?);
                i += 4;
                Some((k >> 3, 5, u64::from(v), &b[0..0]))
            }
            _ => None,
        }
    })
}

fn pb_field(b: &[u8], field: u64) -> Option<&[u8]> {
    pb_fields(b).find(|f| f.0 == field && f.1 == 2).map(|f| f.3)
}

fn pb_varint_field(b: &[u8], field: u64) -> Option<u64> {
    pb_fields(b).find(|f| f.0 == field && f.1 == 0).map(|f| f.2)
}

struct Response {
    status: u16,
    retry_after: Option<u64>,
    body: Vec<u8>,
}

fn http_post(
    host: &str,
    path: &str,
    content_type: &str,
    gzip: bool,
    body: &[u8],
) -> Result<Response, String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    let addr = host
        .to_socket_addrs()
        .map_err(|e| format!("{host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{host}: no address"))?;
    let mut c = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("{host}: {e}"))?;
    c.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: {content_type}\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
        if gzip { "Content-Encoding: gzip\r\n" } else { "" },
        body.len()
    );
    c.write_all(head.as_bytes())
        .and_then(|()| c.write_all(body))
        .map_err(|e| format!("{host}: send: {e}"))?;
    let mut response = Vec::new();
    let _ = c.read_to_end(&mut response);
    parse_response(&response).ok_or_else(|| format!("{host}: no HTTP response"))
}

fn parse_response(response: &[u8]) -> Option<Response> {
    let end = response.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&response[..end]);
    let mut lines = head.lines();
    let status = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let mut retry_after = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            // Seconds; an HTTP-date is not read (backoff applies).
            "retry-after" => retry_after = value.parse().ok(),
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            _ => {}
        }
    }
    let mut body = response[end + 4..].to_vec();
    if chunked {
        body = dechunk(&body);
    }
    Some(Response { status, retry_after, body })
}

/// A chunked body's data (what arrived of it).
fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(eol) = b.windows(2).position(|w| w == b"\r\n") {
        let size = String::from_utf8_lossy(&b[..eol]);
        let size = size.split(';').next().unwrap_or("").trim();
        let Ok(n) = usize::from_str_radix(size, 16) else { break };
        if n == 0 {
            break;
        }
        let data = &b[eol + 2..];
        out.extend_from_slice(&data[..n.min(data.len())]);
        b = data.get(n + 2..).unwrap_or(&[]);
    }
    out
}
