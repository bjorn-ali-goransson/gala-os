//! `cargo xtask agent-relay`: the agent panel's model calls (ROADMAP LY
//! "Change this program", step 4; ARCHITECTURE "The agent panel", Relay).
//!
//! QEMU runs it for each connection to 10.0.2.101:80 (`guestfwd`, as
//! `serve-page`), its standard input and output the connection. The
//! panel POSTs a chat request without a model to
//! `/v1/chat/completions`; the relay puts the model in (`MODEL`, or
//! `XTASK_AGENT_MODEL`) and then either
//!
//! - replays (the default): answers from the recording
//!   `xtask/agent-recordings/<hash>.response.json`, never the network.
//!   A request that is not recorded is answered 502 with a body naming
//!   its hash and the recorded request nearest to it (the byte where
//!   they part), so a changed prompt or tool is seen, not quietly sent;
//!   or
//! - records (`XTASK_AGENT_RECORD=1`): sends it to the API through the
//!   development host's proxy, which adds the key (the relay sends the
//!   placeholder `proxy-injected`), and stores the exchange.
//!
//! The hash is FNV-1a 64 of the request as sent to the API (the model
//! in it). A recording is the request (`<hash>.request.json`) and the
//! API's answer (`<hash>.response.json`), as bytes: no key, no headers.
//! A recording is redone on purpose when the prompts or tools change,
//! and its diff is read like code.
//!
//! Each exchange is a line in `XTASK_AGENT_RELAY_LOG` (the run adds it
//! to the serial log): `xtask: agent-relay: <hash> replayed|recorded|not
//! recorded (nearest <hash> parts at byte <n>)|...`.

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The model the relay asks for unless `XTASK_AGENT_MODEL` names
/// another (it is part of what a recording is keyed by).
pub const MODEL: &str = "gpt-4.1-mini";
/// The API the relay records from.
const API: &str = "https://api.openai.com/v1/chat/completions";
/// The guest's address for the relay (port 80).
pub const GUEST_ADDR: &str = "10.0.2.101";
/// The largest request the relay reads.
const MAX_BODY: usize = 1 << 20;

/// FNV-1a 64 of `bytes`, as 16 hex digits.
pub fn hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The request as sent to the API: `"model"` put first in the body's
/// object unless the panel named one. None when the body is not an
/// object.
pub fn with_model(body: &[u8], model: &str) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(body).ok()?;
    let rest = text.trim_start().strip_prefix('{')?;
    let rest_trim = rest.trim_start();
    // Named by the panel: its first key (a "model" deeper in, a tool's
    // parameter, is not the request's).
    if rest_trim.starts_with("\"model\"") {
        return Some(body.to_vec());
    }
    let sep = if rest_trim.starts_with('}') { "" } else { "," };
    let mut out = format!("{{\"model\":\"{model}\"{sep}");
    out.push_str(rest);
    Some(out.into_bytes())
}

/// Where the recordings are: `XTASK_AGENT_RECORDINGS`, else the
/// repository's `xtask/agent-recordings`.
pub fn recordings() -> PathBuf {
    env::var_os("XTASK_AGENT_RECORDINGS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("agent-recordings"))
}

/// The recorded request nearest to `request` in `dir`: its hash and the
/// byte where the two part (the longest common start).
pub fn nearest(dir: &Path, request: &[u8]) -> Option<(String, usize)> {
    let mut best: Option<(String, usize)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(h) = name.strip_suffix(".request.json") else { continue };
        let Ok(other) = fs::read(entry.path()) else { continue };
        let common = request.iter().zip(&other).take_while(|(a, b)| a == b).count();
        if best.as_ref().is_none_or(|(_, n)| common > *n) {
            best = Some((h.to_string(), common));
        }
    }
    best
}

/// What the relay does with one request.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    /// 200 with the recorded response.
    Replayed(Vec<u8>),
    /// Not in the recording: 502, and why.
    NotRecorded(String),
}

/// Replay: the recording of `request` (with its model) in `dir`.
pub fn replay(dir: &Path, request: &[u8]) -> (String, Answer) {
    let h = hash(request);
    match fs::read(dir.join(format!("{h}.response.json"))) {
        Ok(body) => (h, Answer::Replayed(body)),
        Err(_) => {
            let why = match nearest(dir, request) {
                Some((n, at)) => format!("{h} not recorded (nearest {n} parts at byte {at})"),
                None => format!("{h} not recorded (no recordings in {})", dir.display()),
            };
            (h, Answer::NotRecorded(why))
        }
    }
}

/// Stores an exchange under the request's hash.
pub fn store(dir: &Path, request: &[u8], response: &[u8]) -> Result<String, String> {
    let h = hash(request);
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (suffix, bytes) in [("request", request), ("response", response)] {
        let path = dir.join(format!("{h}.{suffix}.json"));
        fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(h)
}

/// Record: the request sent to the API through the host's proxy (curl,
/// which takes the proxy from the environment; the proxy adds the key):
/// the status and the body.
fn call_api(request: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let dir = env::temp_dir().join(format!("gala-agent-relay-{}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (req, resp) = (dir.join("request.json"), dir.join("response.json"));
    fs::write(&req, request).map_err(|e| e.to_string())?;
    let out = Command::new("curl")
        .args(["-sS", "--max-time", "120", "-X", "POST", API])
        .args(["-H", "Authorization: Bearer proxy-injected"])
        .args(["-H", "Content-Type: application/json"])
        .arg("--data-binary")
        .arg(format!("@{}", req.display()))
        .arg("-o")
        .arg(&resp)
        .args(["-w", "%{http_code}"])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    let body = fs::read(&resp).unwrap_or_default();
    let _ = fs::remove_dir_all(&dir);
    if !out.status.success() {
        return Err(format!("curl: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let code = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
    Ok((code, body))
}

/// Reads an HTTP request from `input`: its request line and its body
/// (Content-Length bytes).
fn read_request(input: &mut impl Read) -> Result<(String, Vec<u8>), String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 64 * 1024 {
            return Err("a request head over 64 KiB".into());
        }
        match input.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return Err("the connection ended in the request's head".into()),
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let first = head.lines().next().unwrap_or("").to_string();
    let length = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if length > MAX_BODY {
        return Err(format!("a body of {length} bytes (at most {MAX_BODY})"));
    }
    let mut body = vec![0u8; length];
    input.read_exact(&mut body).map_err(|e| format!("the body: {e}"))?;
    Ok((first, body))
}

fn respond(out: &mut impl Write, status: u16, body: &[u8]) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Bad Gateway",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    out.write_all(head.as_bytes())
        .and_then(|()| out.write_all(body))
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}

fn error_body(message: &str) -> Vec<u8> {
    let escaped: String = message
        .chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            c if (c as u32) < 0x20 => vec![' '],
            c => vec![c],
        })
        .collect();
    format!("{{\"error\":{{\"message\":\"agent-relay: {escaped}\"}}}}").into_bytes()
}

fn log(line: &str) {
    if let Ok(path) = env::var("XTASK_AGENT_RELAY_LOG")
        && let Ok(mut f) = fs::OpenOptions::new().append(true).create(true).open(path)
    {
        let _ = writeln!(f, "xtask: agent-relay: {line}");
    }
}

/// `cargo xtask agent-relay`: one connection on standard input and
/// output.
pub fn serve() -> Result<(), String> {
    let mut input = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    let (first, body) = match read_request(&mut input) {
        Ok(r) => r,
        Err(e) => {
            log(&format!("refused: {e}"));
            return respond(&mut out, 400, &error_body(&e));
        }
    };
    if !first.starts_with("POST /v1/chat/completions ") {
        log(&format!("refused: {first:?}"));
        return respond(&mut out, 404, &error_body(&format!("no such call: {first}")));
    }
    let model = env::var("XTASK_AGENT_MODEL").unwrap_or_else(|_| MODEL.to_string());
    let Some(request) = with_model(&body, &model) else {
        log("refused: the body is not a JSON object");
        return respond(&mut out, 400, &error_body("the body is not a JSON object"));
    };
    let dir = recordings();
    if env::var_os("XTASK_AGENT_RECORD").is_some_and(|v| v == "1") {
        match call_api(&request) {
            Ok((200, response)) => {
                let h = store(&dir, &request, &response)?;
                log(&format!("{h} recorded ({} bytes in, {} out)", request.len(), response.len()));
                respond(&mut out, 200, &response)
            }
            Ok((code, response)) => {
                log(&format!("{} not recorded: the API answered {code}", hash(&request)));
                respond(&mut out, 502, &response)
            }
            Err(e) => {
                log(&format!("{} not recorded: {e}", hash(&request)));
                respond(&mut out, 502, &error_body(&e))
            }
        }
    } else {
        match replay(&dir, &request) {
            (h, Answer::Replayed(response)) => {
                log(&format!("{h} replayed ({} bytes)", response.len()));
                respond(&mut out, 200, &response)
            }
            (_, Answer::NotRecorded(why)) => {
                log(&why);
                respond(&mut out, 502, &error_body(&why))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_goes_first_unless_named() {
        assert_eq!(
            with_model(b"{\"messages\":[]}", "m").unwrap(),
            b"{\"model\":\"m\",\"messages\":[]}"
        );
        assert_eq!(with_model(b" { }", "m").unwrap(), b"{\"model\":\"m\" }");
        assert_eq!(with_model(b"{\"model\":\"x\"}", "m").unwrap(), b"{\"model\":\"x\"}");
        assert_eq!(with_model(b"[1]", "m"), None);
    }

    #[test]
    fn the_hash_is_fnv1a() {
        assert_eq!(hash(b""), "cbf29ce484222325");
        assert_eq!(hash(b"a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn replays_what_was_recorded_and_names_the_nearest_miss() {
        let dir = env::temp_dir().join(format!("agent-relay-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (h, a) = replay(&dir, b"{\"model\":\"m\",\"messages\":[1]}");
        assert!(matches!(a, Answer::NotRecorded(ref w) if w.contains("no recordings")), "{a:?}");
        assert_eq!(h, hash(b"{\"model\":\"m\",\"messages\":[1]}"));
        let stored = store(&dir, b"{\"model\":\"m\",\"messages\":[1]}", b"{\"ok\":1}").unwrap();
        assert_eq!(stored, h);
        assert_eq!(
            replay(&dir, b"{\"model\":\"m\",\"messages\":[1]}").1,
            Answer::Replayed(b"{\"ok\":1}".to_vec())
        );
        // One message changed: missed, the recording named with where
        // they part.
        let (_, a) = replay(&dir, b"{\"model\":\"m\",\"messages\":[2]}");
        let want = format!("(nearest {h} parts at byte 25)");
        assert!(matches!(a, Answer::NotRecorded(ref w) if w.ends_with(&want)), "{a:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_a_request_and_its_body() {
        let raw = b"POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\ncontent-length: 4\r\n\r\n{}{}";
        let (first, body) = read_request(&mut &raw[..]).unwrap();
        assert_eq!(first, "POST /v1/chat/completions HTTP/1.1");
        assert_eq!(body, b"{}{}");
        assert!(read_request(&mut &b"GET / HTTP/1.1\r\n"[..]).is_err());
    }

    #[test]
    fn an_error_body_is_json() {
        assert_eq!(
            error_body("a \"b\"\n\\"),
            b"{\"error\":{\"message\":\"agent-relay: a \\\"b\\\" \\\\\"}}".to_vec()
        );
    }
}
