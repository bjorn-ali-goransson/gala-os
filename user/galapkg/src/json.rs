//! JSON (RFC 8259) as far as directory listings over HTTP need it: the
//! GitHub REST API's "Get repository content" answer for a directory (an
//! array of objects with `name` and `type`, `"dir"` or `"file"`: GitHub's
//! REST API documentation) and Nginx's `autoindex_format json` (an array
//! of objects with `name` and `type`, `"directory"` or `"file"`: Nginx's
//! ngx_http_autoindex_module documentation), which a mirror of a
//! winget-pkgs tree served over HTTP(S) answers with.
//!
//! The whole grammar is read (objects, arrays, strings with every escape
//! and UTF-16 surrogate pairs, numbers kept as their text, `true`,
//! `false`, `null`); errors name the byte offset. Untrusted input never
//! panics or recurses without bound: nesting deeper than [`MAX_DEPTH`] is
//! an error.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Deeper nesting is refused (a listing is two levels deep; GitHub's
/// `_links` three).
pub const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    /// A number, as written.
    Number(String),
    Str(String),
    Array(Vec<Json>),
    /// Members in order (a repeated name is kept twice).
    Object(Vec<(String, Json)>),
}

impl Json {
    /// A member's value (the first of that name).
    pub fn get(&self, name: &str) -> Option<&Json> {
        match self {
            Json::Object(m) => m.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Parser<'a> {
    b: &'a [u8],
    at: usize,
}

/// Reads one JSON text (whitespace around it allowed, nothing after it).
pub fn parse(text: &str) -> Result<Json, String> {
    let mut p = Parser { b: text.as_bytes(), at: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.at != p.b.len() {
        return Err(p.error("text after the value"));
    }
    Ok(v)
}

impl Parser<'_> {
    fn error(&self, what: &str) -> String {
        format!("JSON at byte {}: {what}", self.at)
    }

    fn ws(&mut self) {
        while self.at < self.b.len() && matches!(self.b[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.at).copied()
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", c as char)))
        }
    }

    fn literal(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.b[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(v)
        } else {
            Err(self.error("not a JSON value"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deep"));
        }
        match self.peek() {
            None => Err(self.error("the text ends where a value should be")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("not a JSON value")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.at += 1;
        let mut members = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a member name"));
            }
            let name = self.string()?;
            self.ws();
            self.expect(b':')?;
            self.ws();
            let v = self.value(depth + 1)?;
            members.push((name, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Json::Object(members));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.at += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        let digits = |p: &mut Self| {
            let s = p.at;
            while p.peek().is_some_and(|c| c.is_ascii_digit()) {
                p.at += 1;
            }
            p.at > s
        };
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => return Err(self.error("a number without digits")),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if !digits(self) {
                return Err(self.error("no digits after '.'"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !digits(self) {
                return Err(self.error("no digits in the exponent"));
            }
        }
        // ASCII only, so a valid str slice.
        Ok(Json::Number(String::from_utf8_lossy(&self.b[start..self.at]).to_string()))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut v = 0;
        for _ in 0..4 {
            let c = self.peek().ok_or_else(|| self.error("a short \\u escape"))?;
            let d = (c as char).to_digit(16).ok_or_else(|| self.error("a bad \\u escape"))?;
            v = v * 16 + d;
            self.at += 1;
        }
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let start = self.at;
            while self.peek().is_some_and(|c| c != b'"' && c != b'\\' && c >= 0x20) {
                self.at += 1;
            }
            // The input is a &str: a run between ASCII delimiters is UTF-8.
            out.push_str(core::str::from_utf8(&self.b[start..self.at]).unwrap_or(""));
            match self.peek() {
                None => return Err(self.error("an unterminated string")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let c = self.peek().ok_or_else(|| self.error("an unterminated escape"))?;
                    self.at += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xd800..0xdc00).contains(&hi)
                                && self.b[self.at..].starts_with(b"\\u")
                            {
                                self.at += 2;
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return Err(self.error("a lone high surrogate"));
                                }
                                0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                            } else {
                                hi
                            };
                            out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(self.error("an unknown escape")),
                    }
                }
                Some(_) => return Err(self.error("a control character in a string")),
            }
        }
    }
}

/// A directory listing's entries (name, is a directory) from GitHub's
/// contents answer or Nginx's JSON autoindex; other entry types
/// (symbolic links, submodules) are left out.
pub fn listing(text: &str) -> Result<Vec<(String, bool)>, String> {
    let Json::Array(items) = parse(text)? else {
        return Err("the listing is not a JSON array (a file, not a directory?)".to_string());
    };
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let name = item
            .get("name")
            .and_then(Json::as_str)
            .ok_or_else(|| format!("listing entry {i} has no name"))?;
        match item.get("type").and_then(Json::as_str) {
            Some("dir" | "directory") => out.push((name.to_string(), true)),
            Some("file") => out.push((name.to_string(), false)),
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn values() {
        assert_eq!(parse(" null ").unwrap(), Json::Null);
        assert_eq!(
            parse("[true,false,-1.5e+3,0]").unwrap(),
            Json::Array(vec![
                Json::Bool(true),
                Json::Bool(false),
                Json::Number("-1.5e+3".into()),
                Json::Number("0".into()),
            ])
        );
        assert_eq!(
            parse(r#""a\"b\\c\/\n\u00e9\ud83d\ude00""#).unwrap(),
            Json::Str("a\"b\\c/\né\u{1f600}".into())
        );
        let o = parse(r#"{"a": {"b": [1, {"c": "d"}]}, "e": ""}"#).unwrap();
        assert_eq!(o.get("e").and_then(Json::as_str), Some(""));
        assert!(o.get("a").and_then(|a| a.get("b")).is_some());
    }

    #[test]
    fn errors_name_the_offset() {
        for (text, msg) in [
            ("", "byte 0: the text ends"),
            ("[1,]", "byte 3: not a JSON value"),
            ("{\"a\" 1}", "byte 5: expected ':'"),
            ("\"ab", "byte 3: an unterminated string"),
            ("01", "byte 1: text after the value"),
            ("[1] x", "byte 4: text after the value"),
            ("\"\\x\"", "an unknown escape"),
            ("\"\\ud800\\u0041\"", "a lone high surrogate"),
            ("1.", "no digits after '.'"),
            ("\"a\u{1}\"", "a control character"),
        ] {
            let e = parse(text).unwrap_err();
            assert!(e.contains(msg), "{text:?}: {e}");
        }
        let deep = "[".repeat(MAX_DEPTH + 2);
        assert!(parse(&deep).unwrap_err().contains("nested too deep"));
        // Every prefix of a valid text fails cleanly.
        let text = r#"[{"name":"a\u00e9","type":"dir","_links":{"self":"x"}},{"n":1.5e-3}]"#;
        for i in 0..text.len() {
            if text.is_char_boundary(i) {
                let _ = parse(&text[..i]);
            }
        }
    }

    #[test]
    fn github_and_nginx_listings() {
        // The shape of GitHub's "Get repository content" for a directory.
        let gh = r#"[
  {"name": "1.8.2", "path": "manifests/j/jqlang/jq/1.8.2", "sha": "0a1b", "size": 0,
   "url": "https://api.github.com/repos/microsoft/winget-pkgs/contents/manifests/j/jqlang/jq/1.8.2?ref=master",
   "html_url": "https://github.com/microsoft/winget-pkgs/tree/master/manifests/j/jqlang/jq/1.8.2",
   "git_url": "https://api.github.com/repos/microsoft/winget-pkgs/git/trees/0a1b",
   "download_url": null, "type": "dir",
   "_links": {"self": "https://api.github.com/...", "git": "https://api.github.com/...", "html": "https://github.com/..."}},
  {"name": "jqlang.jq.yaml", "path": "manifests/j/jqlang/jq/1.8.2/jqlang.jq.yaml", "size": 123,
   "download_url": "https://raw.githubusercontent.com/microsoft/winget-pkgs/master/manifests/j/jqlang/jq/1.8.2/jqlang.jq.yaml",
   "type": "file"},
  {"name": "sub", "type": "submodule"}
]"#;
        assert_eq!(
            listing(gh).unwrap(),
            vec![("1.8.2".to_string(), true), ("jqlang.jq.yaml".to_string(), false)]
        );
        // Nginx's autoindex_format json.
        let nx = r#"[
{ "name":"1.0.0", "type":"directory", "mtime":"Tue, 03 Nov 2026 00:00:00 GMT" },
{ "name":"GalaTest.PkHttps.yaml", "type":"file", "mtime":"Tue, 03 Nov 2026 00:00:00 GMT", "size":512 }
]"#;
        assert_eq!(
            listing(nx).unwrap(),
            vec![("1.0.0".to_string(), true), ("GalaTest.PkHttps.yaml".to_string(), false)]
        );
        // A file's contents answer is an object.
        assert!(
            listing(r#"{"name": "x", "type": "file"}"#).unwrap_err().contains("not a JSON array")
        );
        assert!(listing(r#"[{"type": "dir"}]"#).unwrap_err().contains("entry 0 has no name"));
    }
}
