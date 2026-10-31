//! Differential testing: per-program results of the user test programs on
//! one platform (gala or Windows), stored as JSON, and their comparison.
//!
//! Schema (written by `cargo xtask report` and by `user/bin/run-all.ps1`):
//! `{"platform": "...", "results": [{"program": "x", "exit": 0, "line": "..."}]}`
//! with `exit` null (and an optional `"skipped": "reason"`) for programs
//! that did not run. Programs report through their exit status (0, the
//! number of the failing check, or an NTSTATUS for crashes), so that is
//! what is compared; `line` is the program's own report line, for humans.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub program: String,
    pub exit: Option<u32>,
    pub line: String,
    pub skipped: Option<String>,
}

/// Extracts a program's outcome from a gala serial log.
pub fn outcome_from_serial(program: &str, serial: &str) -> Outcome {
    let exited = format!("process {program}.exe (pid ");
    let exit = serial.lines().find_map(|l| {
        let rest = &l[l.find(&exited)?..];
        let hex = rest.split("exited with status 0x").nth(1)?;
        u32::from_str_radix(hex.get(..8)?, 16).ok()
    });
    let prefix = format!("{program}: ");
    let line = serial
        .lines()
        .find(|l| l.starts_with(&prefix))
        .or_else(|| serial.lines().find(|l| !l.starts_with('[') && l.contains(&prefix)))
        .unwrap_or("")
        .trim_end()
        .to_string();
    Outcome { program: program.to_string(), exit, line, skipped: None }
}

fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
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

pub fn to_json(platform: &str, results: &[Outcome]) -> String {
    let mut out = String::from("{\n  \"platform\": ");
    escape(platform, &mut out);
    out.push_str(",\n  \"results\": [\n");
    for (i, r) in results.iter().enumerate() {
        out.push_str("    {\"program\": ");
        escape(&r.program, &mut out);
        match r.exit {
            Some(e) => {
                let _ = write!(out, ", \"exit\": {e}");
            }
            None => out.push_str(", \"exit\": null"),
        }
        out.push_str(", \"line\": ");
        escape(&r.line, &mut out);
        if let Some(reason) = &r.skipped {
            out.push_str(", \"skipped\": ");
            escape(reason, &mut out);
        }
        out.push('}');
        out.push_str(if i + 1 < results.len() { ",\n" } else { "\n" });
    }
    out.push_str("  ]\n}\n");
    out
}

/// A parsed JSON value (just what the schema needs).
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
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
        match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut map = BTreeMap::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Object(map));
                }
                loop {
                    self.ws();
                    let Json::String(key) = self.value()? else {
                        return Err(format!("object key expected at byte {}", self.i));
                    };
                    self.eat(b':')?;
                    map.insert(key, self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Object(map));
                        }
                        _ => return Err(format!("expected ',' or '}}' at byte {}", self.i)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err(format!("expected ',' or ']' at byte {}", self.i)),
                    }
                }
            }
            Some(b'"') => self.string().map(Json::String),
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
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                while self.i < self.s.len()
                    && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    self.i += 1;
                }
                let text =
                    std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
                text.parse().map(Json::Number).map_err(|_| format!("bad number {text:?}"))
            }
            _ => Err(format!("unexpected input at byte {}", self.i)),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let Some(&c) = self.s.get(self.i) else { return Err("unterminated string".into()) };
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or("unterminated escape")?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self.s.get(self.i..self.i + 4).ok_or("short \\u escape")?;
                            let hex = std::str::from_utf8(hex).map_err(|e| e.to_string())?;
                            let code = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
                            self.i += 4;
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(format!("bad escape at byte {}", self.i)),
                    }
                }
                _ => {
                    // Copy a whole UTF-8 sequence.
                    let start = self.i - 1;
                    let len = match c {
                        0xf0..=0xff => 4,
                        0xe0..=0xef => 3,
                        0xc0..=0xdf => 2,
                        _ => 1,
                    };
                    let bytes = self.s.get(start..start + len).ok_or("bad UTF-8")?;
                    out.push_str(std::str::from_utf8(bytes).map_err(|e| e.to_string())?);
                    self.i = start + len;
                }
            }
        }
    }
}

/// Parses a report file: (platform, results).
pub fn from_json(text: &str) -> Result<(String, Vec<Outcome>), String> {
    // PowerShell's Out-File/Set-Content may write a UTF-8 BOM.
    let text = text.trim_start_matches('\u{feff}');
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let Json::Object(top) = p.value()? else { return Err("top level is not an object".into()) };
    let platform = match top.get("platform") {
        Some(Json::String(s)) => s.clone(),
        _ => return Err("missing \"platform\"".into()),
    };
    let items = match top.get("results") {
        Some(Json::Array(a)) => a.clone(),
        // ConvertTo-Json turns a one-element array into the element.
        Some(o @ Json::Object(_)) => vec![o.clone()],
        _ => return Err("missing \"results\"".into()),
    };
    let mut results = Vec::new();
    for item in items {
        let Json::Object(o) = item else { return Err("result is not an object".into()) };
        let text = |k: &str| match o.get(k) {
            Some(Json::String(s)) => Some(s.clone()),
            _ => None,
        };
        let program = text("program").ok_or("result without \"program\"")?;
        let exit = match o.get("exit") {
            Some(Json::Number(n)) if *n >= 0.0 && *n <= f64::from(u32::MAX) => Some(*n as u32),
            Some(Json::Number(n)) if *n < 0.0 && *n >= f64::from(i32::MIN) => {
                Some(*n as i32 as u32)
            }
            _ => None,
        };
        results.push(Outcome {
            program,
            exit,
            line: text("line").unwrap_or_default(),
            skipped: text("skipped"),
        });
    }
    Ok((platform, results))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Same,
    Different,
    /// Did not run (or is absent) on one side.
    Missing,
}

/// One row per program in either report, sorted by name.
pub fn compare(
    a: &[Outcome],
    b: &[Outcome],
) -> Vec<(String, Option<Outcome>, Option<Outcome>, Verdict)> {
    let mut names: Vec<&str> = a.iter().chain(b).map(|o| o.program.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let x = a.iter().find(|o| o.program == name).cloned();
            let y = b.iter().find(|o| o.program == name).cloned();
            let verdict = match (x.as_ref().and_then(|o| o.exit), y.as_ref().and_then(|o| o.exit)) {
                (Some(p), Some(q)) if p == q => Verdict::Same,
                (Some(_), Some(_)) => Verdict::Different,
                _ => Verdict::Missing,
            };
            (name.to_string(), x, y, verdict)
        })
        .collect()
}

/// A Markdown table of one report.
pub fn markdown(platform: &str, results: &[Outcome]) -> String {
    let mut out =
        format!("# Test programs on {platform}\n\n| program | exit | report |\n|---|---|---|\n");
    for r in results {
        let exit = r.exit.map_or_else(|| "-".to_string(), |e| format!("{e:#010x}"));
        let line = r.skipped.as_deref().map_or_else(|| r.line.clone(), |s| format!("skipped: {s}"));
        let _ = writeln!(out, "| {} | {exit} | {} |", r.program, line.replace('|', "\\|"));
    }
    out
}

/// One kind of missing feature, merged over programs (from the kernel's
/// `ledger: {...}` lines).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LedgerEntry {
    pub service: String,
    pub what: String,
    pub detail: u64,
    pub hits: u64,
    pub programs: Vec<String>,
}

/// Adds the ledger lines in `serial` (from `program`) to `ledger`.
pub fn merge_ledger(ledger: &mut Vec<LedgerEntry>, program: &str, serial: &str) {
    for line in serial.lines() {
        let Some(json) = line.split_once("ledger: ").map(|(_, j)| j.trim()) else { continue };
        let mut p = Parser { s: json.as_bytes(), i: 0 };
        let Ok(Json::Object(o)) = p.value() else { continue };
        let text = |k: &str| match o.get(k) {
            Some(Json::String(s)) => s.clone(),
            _ => String::new(),
        };
        let number = |k: &str| match o.get(k) {
            Some(Json::Number(n)) if *n >= 0.0 => *n as u64,
            _ => 0,
        };
        let (service, what, detail) = (text("service"), text("what"), number("detail"));
        let hits = number("hits");
        match ledger
            .iter_mut()
            .find(|e| e.service == service && e.what == what && e.detail == detail)
        {
            Some(e) => {
                e.hits += hits;
                if !e.programs.iter().any(|p| p == program) {
                    e.programs.push(program.to_string());
                }
            }
            None => ledger.push(LedgerEntry {
                service,
                what,
                detail,
                hits,
                programs: vec![program.to_string()],
            }),
        }
    }
}

/// The ledger as a markdown table, most programs and hits first.
pub fn ledger_markdown(ledger: &[LedgerEntry]) -> String {
    let mut rows = ledger.to_vec();
    rows.sort_by(|a, b| {
        (b.programs.len(), b.hits).cmp(&(a.programs.len(), a.hits)).then(a.service.cmp(&b.service))
    });
    let mut out = String::from(
        "# Missing features (gala ledger)\n\n| service | what | detail | hits | programs |\n|---|---|---|---|---|\n",
    );
    for e in &rows {
        out += &format!(
            "| {} | {} | {:#x} | {} | {} |\n",
            e.service,
            e.what,
            e.detail,
            e.hits,
            e.programs.join(", ")
        );
    }
    if rows.is_empty() {
        out += "| (nothing missing) | | | | |\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(program: &str, exit: Option<u32>) -> Outcome {
        Outcome { program: program.into(), exit, line: String::new(), skipped: None }
    }

    #[test]
    fn serial_logs() {
        let serial = "[info ] run: /boot/vm_test.exe\nvm_test: check 00000003 got c0000005\r\n\
                      [info ] process vm_test.exe (pid 4) exited with status 0x00000003\n";
        let o = outcome_from_serial("vm_test", serial);
        assert_eq!(o.exit, Some(3));
        assert_eq!(o.line, "vm_test: check 00000003 got c0000005");
        let crash = "[warn ] user exception\n[info ] process crash.exe (pid 4) exited with status 0xc0000005\n";
        assert_eq!(outcome_from_serial("crash", crash).exit, Some(0xc000_0005));
        assert_eq!(outcome_from_serial("missing", crash).exit, None);
    }

    #[test]
    fn json_round_trip() {
        let results = vec![
            Outcome {
                program: "a_test".into(),
                exit: Some(0),
                line: "a_test: ok \"quoted\"\\".into(),
                skipped: None,
            },
            Outcome {
                program: "b".into(),
                exit: None,
                line: String::new(),
                skipped: Some("needs gala".into()),
            },
            outcome("crash", Some(0xc000_0005)),
        ];
        let text = to_json("gala", &results);
        assert_eq!(from_json(&text).unwrap(), ("gala".to_string(), results));
    }

    #[test]
    fn powershell_output() {
        // ConvertTo-Json style: BOM, indentation, \u escapes, signed exit codes.
        let text = "\u{feff}{\r\n    \"platform\":  \"windows\",\r\n    \"results\":  [\r\n \
                    {\"program\": \"x_test\", \"exit\": 0, \"line\": \"x_test: ok\"},\r\n \
                    {\"program\": \"crash\", \"exit\": -1073741819, \"line\": \"it\\u0027s\"}]}";
        let (platform, r) = from_json(text).unwrap();
        assert_eq!(platform, "windows");
        assert_eq!(r[1].exit, Some(0xc000_0005));
        assert_eq!(r[1].line, "it's");
        let single = "{\"platform\": \"windows\", \"results\": {\"program\": \"x\", \"exit\": 1}}";
        assert_eq!(from_json(single).unwrap().1.len(), 1);
        assert!(from_json("{\"results\": []}").is_err());
    }

    #[test]
    fn comparison() {
        let gala = [outcome("a", Some(0)), outcome("b", Some(3)), outcome("c", Some(0))];
        let windows = [outcome("a", Some(0)), outcome("b", Some(0)), outcome("d", Some(0))];
        let rows = compare(&gala, &windows);
        let verdicts: Vec<_> = rows.iter().map(|r| (r.0.as_str(), r.3)).collect();
        assert_eq!(
            verdicts,
            [
                ("a", Verdict::Same),
                ("b", Verdict::Different),
                ("c", Verdict::Missing),
                ("d", Verdict::Missing)
            ]
        );
    }

    #[test]
    fn ledger_merges_and_ranks() {
        let mut ledger = Vec::new();
        let a = r#"[info ] x
ledger: {"service":"NtQ","what":"info class","detail":5,"hits":2}
ledger: {"service":"?","what":"unknown service","detail":4095,"hits":1}"#;
        let b = r#"ledger: {"service":"NtQ","what":"info class","detail":5,"hits":3}"#;
        merge_ledger(&mut ledger, "a", a);
        merge_ledger(&mut ledger, "b", b);
        assert_eq!(ledger.len(), 2);
        assert_eq!(ledger[0].hits, 5);
        assert_eq!(ledger[0].programs, ["a", "b"]);
        let md = ledger_markdown(&ledger);
        let first = md.lines().nth(4).unwrap();
        assert!(first.starts_with("| NtQ | info class | 0x5 | 5 | a, b |"), "{first}");
    }
}
