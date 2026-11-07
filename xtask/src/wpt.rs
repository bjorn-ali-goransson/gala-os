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

/// What serve-page answers for `target` (a request's path) in the tree
/// at `root`: (status, content type, body).
pub fn file(root: &Path, target: &str) -> (u16, &'static str, Vec<u8>) {
    let path = target.split(['?', '#']).next().unwrap_or("/").replace("%20", " ");
    let not_found = (404, "text/plain", b"not found\n".to_vec());
    if path.split('/').any(|part| part == "..") {
        return not_found;
    }
    if path == "/resources/testharnessreport.js" {
        return (200, "text/javascript", REPORTER.as_bytes().to_vec());
    }
    let rel = path.trim_start_matches('/');
    for (page, script) in [(".window.html", ".window.js"), (".any.html", ".any.js")] {
        if let Some(stem) = rel.strip_suffix(page) {
            let js = root.join(format!("{stem}{script}"));
            let name = js.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if js.is_file() {
                return (200, "text/html", wrapper(&js, &name));
            }
        }
    }
    let file = root.join(rel);
    let Ok(body) = fs::read(&file) else { return not_found };
    let kind = match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
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
        _ => "application/octet-stream",
    };
    (200, kind, body)
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

fn run_one(port: u16, id: &str, test: &str) -> Outcome {
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
        if started.elapsed() > Duration::from_secs(300) {
            break None;
        }
        thread::sleep(Duration::from_secs(1));
    };
    let Some(text) = results else {
        return Outcome {
            line: format!("{test}: no results in 300 s"),
            result: result("TIMEOUT", "no results in 300 s".into()),
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
        let outcome = run_one(port, &id, test);
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
        let (status, kind, body) = file(&root, "/dom/nodes/a.html?x=1");
        assert_eq!((status, kind, body), (200, "text/html", b"<p>".to_vec()));
        assert_eq!(file(&root, "/dom/nodes/missing.html").0, 404);
        assert_eq!(file(&root, "/dom/../../etc/passwd").0, 404);
        let (_, kind, reporter) = file(&root, "/resources/testharnessreport.js");
        assert_eq!(kind, "text/javascript");
        assert!(String::from_utf8(reporter).unwrap().contains("__gala_wpt"));
        let (status, kind, page) = file(&root, "/dom/nodes/b.window.html");
        let page = String::from_utf8(page).unwrap();
        assert_eq!((status, kind), (200, "text/html"));
        assert!(page.contains("<title>B</title>"));
        assert!(page.contains("<script src=\"../common.js\"></script>"));
        assert!(page.contains("<script src=\"b.window.js\"></script>"));
        let _ = fs::remove_dir_all(&root);
    }
}
