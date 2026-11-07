//! BR's conformance report: web-platform-tests in Chromium on gala
//! (`chromium_wpt`). xtask fetches WPT at WPT_COMMIT (WPT_DIRS only, a
//! sparse checkout), QEMU forwards the guest's 10.0.2.100:80 to `xtask
//! serve-page`, which serves the tree (`file`), and `client` drives one
//! headless Chromium session through ChromeDriver: each test page is
//! loaded and its testharness results read back through our own
//! testharnessreport.js (`REPORTER`). Each test's line goes into the
//! serial log (`xtask: wpt: <path>: ...`); the run's results into
//! target/wpt/wptreport.json (wptrunner's format, what wpt.fyi takes).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::{Result, webdriver};

/// The WPT revision the report runs (a commit of
/// github.com/web-platform-tests/wpt).
pub const WPT_COMMIT: &str = "dd216d1d4f61489b09f566bec2c52d229baa7f49";
/// What is checked out: the harness and the directories whose tests run.
const WPT_DIRS: &[&str] = &["resources", "dom/nodes"];
/// The directories whose tests run (WPT_DIRS less the harness).
const TEST_DIRS: &[&str] = &["dom/nodes"];

/// The WPT tree at WPT_COMMIT (target/prebuilt/wpt), fetched on first use
/// with the host's git: a shallow, blob-filtered, sparse checkout.
pub fn tree() -> Result<PathBuf> {
    let dir = PathBuf::from("target/prebuilt/wpt");
    let stamp = dir.join(".gala-commit");
    let want = format!("{WPT_COMMIT} {}", WPT_DIRS.join(" "));
    if fs::read_to_string(&stamp).is_ok_and(|s| s == want) {
        return fs::canonicalize(&dir).map_err(|e| e.to_string());
    }
    eprintln!("xtask: fetching web-platform-tests at {WPT_COMMIT} ({})", WPT_DIRS.join(", "));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let git = |args: &[&str]| -> Result {
        let out = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .output()
            .map_err(|e| format!("git: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)))
        }
    };
    git(&["init", "-q"])?;
    git(&["remote", "add", "origin", "https://github.com/web-platform-tests/wpt.git"])?;
    git(&["sparse-checkout", "init", "--cone"])?;
    let mut set = vec!["sparse-checkout", "set"];
    set.extend_from_slice(WPT_DIRS);
    git(&set)?;
    git(&["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", WPT_COMMIT])?;
    git(&["checkout", "-q", "FETCH_HEAD"])?;
    fs::write(&stamp, want).map_err(|e| e.to_string())?;
    fs::canonicalize(&dir).map_err(|e| e.to_string())
}

/// Our testharnessreport.js (as wptrunner's: output off, timeouts
/// multiplied for TCG): the completion callback leaves the results in
/// `window.__gala_wpt` as lines: `HARNESS\t<status>\t<message>`, then
/// `<status>\t<name>\t<message>` per subtest (tabs and newlines in the
/// texts become spaces).
const REPORTER: &str = r#"setup({output: false, timeout_multiplier: 4});
add_completion_callback(function (tests, status) {
  var clean = function (s) {
    return String(s === undefined || s === null ? "" : s).replace(/[\t\n\r]/g, " ");
  };
  var lines = ["HARNESS\t" + status.status + "\t" + clean(status.message)];
  tests.forEach(function (t) {
    lines.push(t.status + "\t" + clean(t.name) + "\t" + clean(t.message));
  });
  window.__gala_wpt = lines.join("\n");
});
"#;

/// The page that runs a `.window.js` or `.any.js` test in a window (as
/// wptserve makes it): the harness, the `// META: script=` scripts, the
/// test.
fn wrapper(js: &Path, name: &str) -> Vec<u8> {
    let source = fs::read_to_string(js).unwrap_or_default();
    let mut scripts = String::new();
    let mut title = String::new();
    for line in source.lines().take_while(|l| l.starts_with("//")) {
        let Some(meta) = line.strip_prefix("// META:") else { continue };
        let meta = meta.trim();
        if let Some(src) = meta.strip_prefix("script=") {
            scripts += &format!("<script src=\"{src}\"></script>\n");
        } else if let Some(t) = meta.strip_prefix("title=") {
            title = format!("<title>{t}</title>\n");
        }
    }
    format!(
        "<!doctype html>\n<meta charset=utf-8>\n{title}<script src=\"/resources/testharness.js\"></script>\n\
         <script src=\"/resources/testharnessreport.js\"></script>\n{scripts}<div id=log></div>\n\
         <script src=\"{name}\"></script>\n"
    )
    .into_bytes()
}

/// What serve-page sends: a status, headers (Content-Type among them),
/// the body.
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn new(status: u16, kind: &str, body: Vec<u8>) -> Response {
        Response { status, headers: vec![("Content-Type".into(), kind.into())], body }
    }

    /// Sets a header (replacing one of the same name).
    fn set(&mut self, name: &str, value: &str) {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.headers.push((name.to_string(), value.to_string()));
    }

    pub fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            204 => "No Content",
            301 => "Moved Permanently",
            302 => "Found",
            304 => "Not Modified",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Status",
        }
    }
}

/// A file's MIME type, as wptserve gives it (by extension).
fn mime(file: &Path) -> &'static str {
    match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" | "htm" => "text/html",
        "xhtml" | "xht" => "application/xhtml+xml",
        "svg" => "image/svg+xml",
        "xml" => "application/xml",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "txt" => "text/plain",
        "png" => "image/png",
        "gif" => "image/gif",
        "jpg" | "jpeg" => "image/jpeg",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// wptserve's `pipe=` options the tests in WPT_DIRS use: `header(name,
/// value)` and `status(code)`, joined with `|`; others are ignored.
fn apply_pipes(response: &mut Response, query: &str) {
    let Some(pipes) = query.split('&').find_map(|p| p.strip_prefix("pipe=")) else { return };
    let pipes = pipes.replace("%2C", ",").replace("%20", " ").replace("%7C", "|");
    for pipe in pipes.split('|') {
        if let Some(args) = pipe.strip_prefix("header(").and_then(|a| a.strip_suffix(')'))
            && let Some((name, value)) = args.split_once(',')
        {
            response.set(name.trim(), value.trim());
        } else if let Some(code) = pipe.strip_prefix("status(").and_then(|a| a.strip_suffix(')'))
            && let Ok(code) = code.trim().parse()
        {
            response.status = code;
        }
    }
}

/// What serve-page answers for `target` (a request's path and query) in
/// the tree at `root`: a file with its MIME type and its `.headers`
/// file's headers, a generated page for `.window.js`/`.any.js` tests,
/// our testharnessreport.js; then the query's `pipe=` options.
pub fn file(root: &Path, target: &str) -> Response {
    let (path, query) =
        target.split('#').next().unwrap_or("/").split_once('?').unwrap_or((target, ""));
    let path = path.replace("%20", " ");
    let not_found = || Response::new(404, "text/plain", b"not found\n".to_vec());
    if path.split('/').any(|part| part == "..") {
        return not_found();
    }
    let mut response = if path == "/resources/testharnessreport.js" {
        Response::new(200, "text/javascript", REPORTER.as_bytes().to_vec())
    } else {
        let rel = path.trim_start_matches('/');
        let generated = [(".window.html", ".window.js"), (".any.html", ".any.js")].iter().find_map(
            |(page, script)| {
                let stem = rel.strip_suffix(page)?;
                let js = root.join(format!("{stem}{script}"));
                let name = js.file_name()?.to_string_lossy().into_owned();
                js.is_file().then(|| Response::new(200, "text/html", wrapper(&js, &name)))
            },
        );
        match generated {
            Some(r) => r,
            None => {
                let file = root.join(rel);
                let Ok(body) = fs::read(&file) else { return not_found() };
                let mut r = Response::new(200, mime(&file), body);
                // `<file>.headers`: one `Name: value` a line.
                let mut headers = file.clone().into_os_string();
                headers.push(".headers");
                if let Ok(text) = fs::read_to_string(&headers) {
                    for line in text.lines() {
                        if let Some((name, value)) = line.split_once(':') {
                            r.set(name.trim(), value.trim());
                        }
                    }
                }
                r
            }
        }
    };
    apply_pipes(&mut response, query);
    response
}

/// The testharness tests under TEST_DIRS, as URL paths: pages that load
/// testharness.js, and `.window.js` / `.any.js` tests as their window
/// pages; not support files, crash tests, manual tests or reftests.
/// XTASK_WPT_FILTER keeps the paths containing it, XTASK_WPT_LIMIT the
/// first so many.
fn tests(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !matches!(name.as_str(), "support" | "resources" | "crashtests" | "tools") {
                    walk(root, &path, out);
                }
                continue;
            }
            if name.contains("-manual") || name.contains("-crash") || name.contains("-ref.") {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            if let Some(stem) = rel.strip_suffix(".window.js") {
                out.push(format!("/{stem}.window.html"));
            } else if let Some(stem) = rel.strip_suffix(".any.js") {
                out.push(format!("/{stem}.any.html"));
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("html" | "htm" | "xhtml" | "svg")
            ) && fs::read_to_string(&path)
                .is_ok_and(|s| s.contains("/resources/testharness.js"))
            {
                out.push(format!("/{rel}"));
            }
        }
    }
    let mut out = Vec::new();
    for dir in TEST_DIRS {
        walk(root, &root.join(dir), &mut out);
    }
    out.sort();
    if let Ok(filter) = std::env::var("XTASK_WPT_FILTER") {
        out.retain(|p| p.contains(&filter));
    }
    if let Some(limit) = std::env::var("XTASK_WPT_LIMIT").ok().and_then(|l| l.parse().ok()) {
        out.truncate(limit);
    }
    out
}

/// A JSON string's value (the text between its quotes, unescaped) at
/// the start of `s`, and what follows it.
fn json_text(s: &str) -> Option<(String, &str)> {
    let mut chars = s.strip_prefix('"')?.char_indices();
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &s[i + 2..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let hex: String = (0..4).filter_map(|_| chars.next().map(|c| c.1)).collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// The `"value"` of a WebDriver answer when it is a string.
fn value_text(body: &str) -> Option<String> {
    let at = body.find("\"value\"")?;
    let rest = body[at + 7..].trim_start().strip_prefix(':')?.trim_start();
    json_text(rest).map(|(v, _)| v)
}

/// testharness's names for a harness status and a subtest status.
fn harness_status(code: &str) -> &'static str {
    match code {
        "0" => "OK",
        "1" => "ERROR",
        "2" => "TIMEOUT",
        "3" => "PRECONDITION_FAILED",
        _ => "UNKNOWN",
    }
}

fn subtest_status(code: &str) -> &'static str {
    match code {
        "0" => "PASS",
        "1" => "FAIL",
        "2" => "TIMEOUT",
        "3" => "NOTRUN",
        "4" => "PRECONDITION_FAILED",
        _ => "UNKNOWN",
    }
}

const CAPABILITIES: &str = r#"{"capabilities":{"alwaysMatch":{"timeouts":{"pageLoad":300000,"script":60000},"goog:chromeOptions":{"binary":"C:\\chromium\\chrome.exe","args":["--headless","--no-sandbox","--disable-gpu","--no-first-run","--user-data-dir=C:\\tmp\\chrw","--enable-logging","--log-file=C:\\chromium\\chrome_driven.log","--ipc-connection-timeout=120"]}}}}"#;

/// A new session's id.
fn session(port: u16) -> Result<String> {
    let (status, body) = webdriver(port, "POST", "/session", CAPABILITIES, 600)?;
    match crate::json_string(&body, "sessionId") {
        Some(id) if status == 200 => Ok(id.to_string()),
        _ => Err(format!("POST /session: {status} {body}")),
    }
}

/// One test's result as wptreport.json has it (wptrunner's format, what
/// wpt.fyi takes): the harness status and message, each subtest's.
pub struct TestResult {
    pub test: String,
    pub status: &'static str,
    pub message: String,
    pub subtests: Vec<(String, &'static str, String)>,
    pub duration_ms: u128,
}

/// One test's outcome: its line for the log and its result.
struct Outcome {
    line: String,
    result: TestResult,
    /// The session ended (the browser went away): start another.
    lost: bool,
}

/// A test's source: its page, or the `.window.js`/`.any.js` it wraps.
fn source(root: &Path, test: &str) -> String {
    let rel = test.trim_start_matches('/');
    let rel = rel
        .strip_suffix(".window.html")
        .map(|s| format!("{s}.window.js"))
        .or_else(|| rel.strip_suffix(".any.html").map(|s| format!("{s}.any.js")))
        .unwrap_or_else(|| rel.to_string());
    fs::read_to_string(root.join(rel)).unwrap_or_default()
}

/// How long to wait for a test's results: testharness's own timeout (10
/// s, 60 s for `<meta name="timeout" content="long">`) times REPORTER's
/// multiplier (4), and 20 s for the harness to report it.
fn wait_for(source: &str) -> Duration {
    let long = source.contains("name=\"timeout\" content=\"long\"")
        || source.contains("META: timeout=long");
    Duration::from_secs(if long { 60 } else { 10 } * 4 + 20)
}

/// Why a test cannot run against serve-page (it needs wptserve's Python
/// handlers: a `.py` resource), if so.
fn unservable(source: &str) -> Option<&'static str> {
    source.contains(".py").then_some("needs wptserve's Python handlers (serve-page serves files)")
}

fn run_one(port: u16, id: &str, test: &str, wait: Duration) -> Outcome {
    let started = std::time::Instant::now();
    let result = |status: &'static str, message: String| TestResult {
        test: test.to_string(),
        status,
        message,
        subtests: Vec::new(),
        duration_ms: started.elapsed().as_millis(),
    };
    let lost =
        |answer: &str| answer.contains("invalid session id") || answer.contains("disconnected");
    let url = format!("{{\"url\":\"http://10.0.2.100{test}\"}}");
    match webdriver(port, "POST", &format!("/session/{id}/url"), &url, 330) {
        Ok((200, _)) => {}
        Ok((status, answer)) => {
            let message = answer.chars().take(200).collect::<String>();
            return Outcome {
                line: format!("{test}: load failed: {status} {message}"),
                result: result("ERROR", format!("load failed: {status} {message}")),
                lost: lost(&answer),
            };
        }
        Err(e) => {
            return Outcome {
                line: format!("{test}: load failed: {e}"),
                result: result("ERROR", format!("load failed: {e}")),
                lost: true,
            };
        }
    }
    let script = r#"{"script":"return window.__gala_wpt || null","args":[]}"#;
    let results = loop {
        match webdriver(port, "POST", &format!("/session/{id}/execute/sync"), script, 120) {
            Ok((200, answer)) => {
                if let Some(text) = value_text(&answer) {
                    break Some(text);
                }
            }
            Ok((_, answer)) if lost(&answer) => {
                return Outcome {
                    line: format!("{test}: session lost"),
                    result: result("CRASH", "the browser went away".into()),
                    lost: true,
                };
            }
            _ => {}
        }
        if started.elapsed() > wait {
            break None;
        }
        thread::sleep(Duration::from_secs(1));
    };
    let Some(text) = results else {
        return Outcome {
            line: format!("{test}: no results in {} s", wait.as_secs()),
            result: result("TIMEOUT", format!("no results in {} s", wait.as_secs())),
            lost: false,
        };
    };
    let mut lines = text.lines();
    let harness: Vec<&str> = lines.next().unwrap_or("").split('\t').collect();
    let status = harness_status(harness.get(1).copied().unwrap_or(""));
    let mut r = result(status, harness.get(2).copied().unwrap_or("").to_string());
    for line in lines {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        r.subtests.push((
            parts.get(1).copied().unwrap_or("").to_string(),
            subtest_status(parts.first().copied().unwrap_or("")),
            parts.get(2).copied().unwrap_or("").to_string(),
        ));
    }
    let pass = r.subtests.iter().filter(|s| s.1 == "PASS").count();
    Outcome {
        line: format!("{test}: {status} {pass}/{}", r.subtests.len()),
        result: r,
        lost: false,
    }
}

/// A JSON string literal (quoted, escaped).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A message as wptreport has it: null when empty.
fn json_message(s: &str) -> String {
    if s.is_empty() { "null".into() } else { json_str(s) }
}

/// The run as wptreport.json: `run_info` (the browser, gala as the OS,
/// the WPT revision), the start and end (ms since the epoch), each
/// test's result.
pub fn wptreport(results: &[TestResult], time_start: u128, time_end: u128) -> String {
    let mut out = format!(
        "{{\"run_info\":{{\"product\":\"chrome\",\"browser_version\":\"159.0.8140.0\",\
         \"os\":\"gala\",\"os_version\":\"10.0.26100\",\"processor\":\"x86_64\",\
         \"headless\":true,\"revision\":\"{WPT_COMMIT}\"}},\
         \"time_start\":{time_start},\"time_end\":{time_end},\"results\":["
    );
    for (i, r) in results.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out += &format!(
            "{{\"test\":{},\"status\":\"{}\",\"message\":{},\"duration\":{},\"subtests\":[",
            json_str(&r.test),
            r.status,
            json_message(&r.message),
            r.duration_ms
        );
        for (j, (name, status, message)) in r.subtests.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out += &format!(
                "{{\"name\":{},\"status\":\"{status}\",\"message\":{}}}",
                json_str(name),
                json_message(message)
            );
        }
        out += "]}";
    }
    out += "]}\n";
    out
}

fn epoch_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// The host side of `chromium_wpt`: every test in one session (a new one
/// when the browser goes away), each test's line appended to `log` (the
/// run's serve log, which goes into the serial log), the results to
/// target/wpt/wptreport.json, a summary line last; then ChromeDriver's
/// /shutdown ends the run. Failing tests are the report, not a failure:
/// Err only when no session can be made.
pub fn client(port: u16, log: &Path) -> Result {
    let verdict = run(port, log);
    let _ = webdriver(port, "GET", "/shutdown", "", 60);
    verdict
}

fn run(port: u16, log: &Path) -> Result {
    use std::io::Write;
    let note = |line: &str| {
        if let Ok(mut f) = fs::OpenOptions::new().append(true).create(true).open(log) {
            let _ = writeln!(f, "xtask: wpt: {line}");
        }
    };
    let root = tree()?;
    let list = tests(&root);
    note(&format!("{} tests at {WPT_COMMIT} ({})", list.len(), TEST_DIRS.join(", ")));
    for _ in 0..60 {
        if matches!(webdriver(port, "GET", "/status", "", 60), Ok((200, ref b)) if b.contains("\"ready\":true"))
        {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    let time_start = epoch_ms();
    let mut id = session(port)?;
    fs::create_dir_all("target/wpt").map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    for test in &list {
        let text = source(&root, test);
        if let Some(why) = unservable(&text) {
            note(&format!("{test}: SKIP: {why}"));
            results.push(TestResult {
                test: test.clone(),
                status: "SKIP",
                message: why.into(),
                subtests: Vec::new(),
                duration_ms: 0,
            });
            continue;
        }
        let outcome = run_one(port, &id, test, wait_for(&text));
        note(&outcome.line);
        results.push(outcome.result);
        if outcome.lost {
            id = session(port)?;
        }
    }
    let _ = webdriver(port, "DELETE", &format!("/session/{id}"), "", 120);
    fs::write("target/wpt/wptreport.json", wptreport(&results, time_start, epoch_ms()))
        .map_err(|e| e.to_string())?;
    let ok = results.iter().filter(|r| r.status == "OK").count();
    let total: usize = results.iter().map(|r| r.subtests.len()).sum();
    let pass: usize =
        results.iter().map(|r| r.subtests.iter().filter(|s| s.1 == "PASS").count()).sum();
    note(&format!(
        "summary: {} tests, {ok} harness OK, {pass} of {total} subtests pass (target/wpt/wptreport.json)",
        list.len()
    ));
    Ok(())
}

// ---- The comparison with wpt.fyi ----

/// wpt.fyi's runs the report is compared with (the Chromium of our
/// snapshot's line: Edge on Windows, the same Blink on the same OS, and
/// Chrome on Linux), pinned so a comparison can be repeated.
const REFERENCE_RUNS: &[(&str, &str)] =
    &[("edge-159-win", "5094276917362688"), ("chrome-159-linux", "6250565395546112")];

/// A JSON value (enough of JSON for wptreport.json and wpt.fyi's API).
#[derive(Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        if let Json::Str(s) = self { Some(s) } else { None }
    }
    pub fn num(&self) -> Option<f64> {
        if let Json::Num(n) = self { Some(*n) } else { None }
    }
    pub fn arr(&self) -> &[Json] {
        if let Json::Arr(a) = self { a } else { &[] }
    }
}

/// Parses a JSON document.
pub fn parse_json(text: &str) -> Option<Json> {
    let (v, rest) = value(text.trim_start())?;
    rest.trim().is_empty().then_some(v)
}

fn value(s: &str) -> Option<(Json, &str)> {
    let s = s.trim_start();
    match s.chars().next()? {
        '"' => json_text(s).map(|(t, r)| (Json::Str(t), r)),
        '{' => {
            let mut fields = Vec::new();
            let mut s = s[1..].trim_start();
            if let Some(r) = s.strip_prefix('}') {
                return Some((Json::Obj(fields), r));
            }
            loop {
                let (k, r) = json_text(s.trim_start())?;
                let r = r.trim_start().strip_prefix(':')?;
                let (v, r) = value(r)?;
                fields.push((k, v));
                let r = r.trim_start();
                if let Some(r) = r.strip_prefix(',') {
                    s = r;
                } else {
                    return Some((Json::Obj(fields), r.strip_prefix('}')?));
                }
            }
        }
        '[' => {
            let mut items = Vec::new();
            let mut s = s[1..].trim_start();
            if let Some(r) = s.strip_prefix(']') {
                return Some((Json::Arr(items), r));
            }
            loop {
                let (v, r) = value(s)?;
                items.push(v);
                let r = r.trim_start();
                if let Some(r) = r.strip_prefix(',') {
                    s = r;
                } else {
                    return Some((Json::Arr(items), r.strip_prefix(']')?));
                }
            }
        }
        't' => s.strip_prefix("true").map(|r| (Json::Bool(true), r)),
        'f' => s.strip_prefix("false").map(|r| (Json::Bool(false), r)),
        'n' => s.strip_prefix("null").map(|r| (Json::Null, r)),
        _ => {
            let end =
                s.find(|c: char| !(c.is_ascii_digit() || "+-.eE".contains(c))).unwrap_or(s.len());
            s[..end].parse().ok().map(|n| (Json::Num(n), &s[end..]))
        }
    }
}

/// One test on both sides: gala's subtests passed and run (its harness
/// status), a reference run's passes and total.
struct Row {
    test: String,
    gala: (usize, usize, String),
    reference: Vec<Option<(u64, u64)>>,
    /// gala's first failing subtest and its message.
    first_failure: String,
}

/// `cargo xtask wpt-compare`: target/wpt/wptreport.json against
/// REFERENCE_RUNS on wpt.fyi (its search API, by test: passes and total,
/// fetched with the host's curl): per directory, and every test that
/// passes fewer subtests on gala than on the first reference (gala-only
/// failures, with gala's first failing subtest). Printed and written to
/// target/wpt/compare.txt.
pub fn compare() -> Result {
    let report = fs::read_to_string("target/wpt/wptreport.json")
        .map_err(|e| format!("target/wpt/wptreport.json: {e} (run chromium_wpt first)"))?;
    let report = parse_json(&report).ok_or("target/wpt/wptreport.json: not JSON")?;
    let mut rows = Vec::new();
    let mut dirs = std::collections::BTreeSet::new();
    for r in report.get("results").map(Json::arr).unwrap_or(&[]) {
        let test = r.get("test").and_then(Json::str).unwrap_or("").to_string();
        let status = r.get("status").and_then(Json::str).unwrap_or("").to_string();
        let subtests = r.get("subtests").map(Json::arr).unwrap_or(&[]);
        let pass =
            subtests.iter().filter(|s| s.get("status").and_then(Json::str) == Some("PASS")).count();
        let first_failure = subtests
            .iter()
            .find(|s| s.get("status").and_then(Json::str) != Some("PASS"))
            .map(|s| {
                format!(
                    "{}: {} {}",
                    s.get("name").and_then(Json::str).unwrap_or(""),
                    s.get("status").and_then(Json::str).unwrap_or(""),
                    s.get("message").and_then(Json::str).unwrap_or("")
                )
            })
            .unwrap_or_default();
        if let Some(dir) = test.rsplit_once('/').map(|(d, _)| d.to_string()) {
            dirs.insert(dir);
        }
        rows.push(Row {
            test,
            gala: (pass, subtests.len(), status),
            reference: vec![None; REFERENCE_RUNS.len()],
            first_failure,
        });
    }
    let ids: Vec<&str> = REFERENCE_RUNS.iter().map(|r| r.1).collect();
    for dir in &dirs {
        let url = format!("https://wpt.fyi/api/search?run_ids={}&q={dir}/", ids.join(","));
        let out = Command::new("curl")
            .args(["-sSf", "--max-time", "120", &url])
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            return Err(format!("{url}: {}", String::from_utf8_lossy(&out.stderr)));
        }
        let answer = parse_json(&String::from_utf8_lossy(&out.stdout))
            .ok_or_else(|| format!("{url}: not JSON"))?;
        for r in answer.get("results").map(Json::arr).unwrap_or(&[]) {
            let test = r.get("test").and_then(Json::str).unwrap_or("");
            let Some(row) = rows.iter_mut().find(|row| row.test == test) else { continue };
            for (i, s) in r.get("legacy_status").map(Json::arr).unwrap_or(&[]).iter().enumerate() {
                if let (Some(p), Some(t)) =
                    (s.get("passes").and_then(Json::num), s.get("total").and_then(Json::num))
                    && i < row.reference.len()
                {
                    row.reference[i] = Some((p as u64, t as u64));
                }
            }
        }
    }
    let mut text = format!(
        "wpt-compare: gala (chromium_wpt, WPT {WPT_COMMIT}) against {}\n",
        REFERENCE_RUNS
            .iter()
            .map(|r| format!("{} (run {})", r.0, r.1))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let sum = |f: &dyn Fn(&Row) -> Option<(u64, u64)>| {
        rows.iter().filter_map(f).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    };
    let gala = sum(&|r| Some((r.gala.0 as u64, r.gala.1 as u64)));
    text += &format!("tests: {}; subtests passed: gala {} of {}", rows.len(), gala.0, gala.1);
    for (i, (name, _)) in REFERENCE_RUNS.iter().enumerate() {
        let r = sum(&|row| row.reference[i]);
        text += &format!(", {name} {} of {}", r.0, r.1);
    }
    text += "\n";
    let mut worse: Vec<&Row> = rows
        .iter()
        .filter(|r| r.reference[0].is_some_and(|(p, _)| (r.gala.0 as u64) < p))
        .collect();
    worse.sort_by(|a, b| a.test.cmp(&b.test));
    text += &format!(
        "gala-only failures (fewer passes than {}): {}\n",
        REFERENCE_RUNS[0].0,
        worse.len()
    );
    for r in &worse {
        let reference = r.reference[0].map_or("?".to_string(), |(p, t)| format!("{p}/{t}"));
        text += &format!(
            "  {}: gala {}/{} ({}), {} {reference}; first failure: {}\n",
            r.test,
            r.gala.0,
            r.gala.1,
            r.gala.2,
            REFERENCE_RUNS[0].0,
            r.first_failure.chars().take(200).collect::<String>()
        );
    }
    let unknown = rows.iter().filter(|r| r.reference[0].is_none()).count();
    if unknown > 0 {
        text += &format!("not in {}: {unknown} tests\n", REFERENCE_RUNS[0].0);
    }
    print!("{text}");
    fs::write("target/wpt/compare.txt", &text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_text_unescapes() {
        let (v, rest) = json_text(r#""a\tb\nc\"d\\eé" ,"x""#).unwrap();
        assert_eq!(v, "a\tb\nc\"d\\eé");
        assert_eq!(rest, r#" ,"x""#);
        assert_eq!(value_text(r#"{"value":"HARNESS\t0\t"}"#).as_deref(), Some("HARNESS\t0\t"));
        assert_eq!(value_text(r#"{"value":null}"#), None);
    }

    #[test]
    fn parses_json() {
        let v = parse_json(
            r#" {"a": [1, 2.5, -3e2], "b": {"c": "d\"e"}, "t": true, "n": null, "e": []} "#,
        )
        .unwrap();
        assert_eq!(
            v.get("a").unwrap().arr().iter().filter_map(Json::num).collect::<Vec<_>>(),
            [1.0, 2.5, -300.0]
        );
        assert_eq!(v.get("b").and_then(|b| b.get("c")).and_then(Json::str), Some("d\"e"));
        assert_eq!(v.get("t"), Some(&Json::Bool(true)));
        assert_eq!(v.get("n"), Some(&Json::Null));
        assert!(v.get("e").unwrap().arr().is_empty());
        assert!(parse_json("{\"a\": }").is_none());
        // Our own report reads back.
        let results = [TestResult {
            test: "/x.html".into(),
            status: "OK",
            message: String::new(),
            subtests: vec![("s".into(), "PASS", String::new())],
            duration_ms: 1,
        }];
        let back = parse_json(&wptreport(&results, 1, 2)).unwrap();
        let r = &back.get("results").unwrap().arr()[0];
        assert_eq!(r.get("test").and_then(Json::str), Some("/x.html"));
    }

    #[test]
    fn writes_wptreport_json() {
        let results = [TestResult {
            test: "/dom/nodes/a.html".into(),
            status: "OK",
            message: String::new(),
            subtests: vec![
                ("first \"one\"".into(), "PASS", String::new()),
                ("second".into(), "FAIL", "assert_equals: got 1\nwant 2".into()),
            ],
            duration_ms: 1234,
        }];
        let json = wptreport(&results, 10, 20);
        assert!(json.contains(r#""time_start":10,"time_end":20"#));
        assert!(json.contains(r#""revision":"dd216d1d4f61489b09f566bec2c52d229baa7f49""#));
        assert!(json.contains(
            r#"{"test":"/dom/nodes/a.html","status":"OK","message":null,"duration":1234,"subtests":[{"name":"first \"one\"","status":"PASS","message":null},{"name":"second","status":"FAIL","message":"assert_equals: got 1\nwant 2"}]}"#
        ));
    }

    #[test]
    fn serves_the_tree_and_its_generated_pages() {
        let root = std::env::temp_dir().join(format!("gala-wpt-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("dom/nodes")).unwrap();
        fs::write(root.join("dom/nodes/a.html"), "<p>").unwrap();
        fs::write(
            root.join("dom/nodes/b.window.js"),
            "// META: title=B\n// META: script=../common.js\ntest(() => {});\n",
        )
        .unwrap();
        let r = file(&root, "/dom/nodes/a.html?x=1");
        assert_eq!((r.status, r.body.as_slice()), (200, b"<p>".as_slice()));
        assert_eq!(r.headers, [("Content-Type".to_string(), "text/html".to_string())]);
        assert_eq!(file(&root, "/dom/nodes/missing.html").status, 404);
        assert_eq!(file(&root, "/dom/../../etc/passwd").status, 404);
        let reporter = file(&root, "/resources/testharnessreport.js");
        assert!(String::from_utf8(reporter.body).unwrap().contains("__gala_wpt"));
        let page = file(&root, "/dom/nodes/b.window.html");
        let text = String::from_utf8(page.body).unwrap();
        assert_eq!(page.status, 200);
        assert!(text.contains("<title>B</title>"));
        assert!(text.contains("<script src=\"../common.js\"></script>"));
        assert!(text.contains("<script src=\"b.window.js\"></script>"));
        // A .headers file and pipes.
        fs::write(root.join("dom/nodes/t.bmp"), "BM").unwrap();
        fs::write(root.join("dom/nodes/t.bmp.headers"), "X-Test: 1\nContent-Type: image/x-test\n")
            .unwrap();
        let bmp = file(&root, "/dom/nodes/t.bmp");
        assert!(bmp.headers.contains(&("Content-Type".into(), "image/x-test".into())));
        assert!(bmp.headers.contains(&("X-Test".into(), "1".into())));
        let piped =
            file(&root, "/dom/nodes/a.html?pipe=header(Content-Type,text/plain)|status(404)");
        assert_eq!(piped.status, 404);
        assert!(piped.headers.contains(&("Content-Type".into(), "text/plain".into())));
        let _ = fs::remove_dir_all(&root);
    }
}
