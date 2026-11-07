//! The JSON TUF's metadata is written in, and its canonical form (the OLPC
//! "Canonical JSON" python-tuf and go-tuf sign): no whitespace, object keys
//! sorted by their bytes, integers only, strings with only `"` and `\`
//! escaped. A signature covers the canonical form of the `signed` value, so
//! a reader re-encodes what it parsed and checks that.
//!
//! The parser takes any JSON (whitespace, every escape) but refuses what
//! canonical JSON cannot carry (fractions, exponents), duplicate keys
//! (two readers could keep different ones) and nesting deeper than
//! [`MAX_DEPTH`].

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// The deepest nesting a document may have (TUF's goes 5 deep).
pub const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<Value>),
    /// `String`'s order is its bytes' order: the canonical key order.
    Object(BTreeMap<String, Value>),
}

/// Why a document is not JSON this module takes: what, and the byte offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    pub what: &'static str,
    pub at: usize,
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(m) => m.get(key),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The canonical form.
    pub fn canonical(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Value::Null => out.extend_from_slice(b"null"),
            Value::Bool(true) => out.extend_from_slice(b"true"),
            Value::Bool(false) => out.extend_from_slice(b"false"),
            Value::Int(n) => write_int(*n, out),
            Value::Str(s) => write_str(s, out),
            Value::Array(a) => {
                out.push(b'[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    v.write(out);
                }
                out.push(b']');
            }
            Value::Object(m) => {
                out.push(b'{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write_str(k, out);
                    out.push(b':');
                    v.write(out);
                }
                out.push(b'}');
            }
        }
    }
}

fn write_int(n: i64, out: &mut Vec<u8>) {
    if n < 0 {
        out.push(b'-');
    }
    let mut digits = [0u8; 20];
    let mut len = 0;
    let mut m = n.unsigned_abs();
    loop {
        digits[len] = b'0' + (m % 10) as u8;
        len += 1;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    out.extend(digits[..len].iter().rev());
}

fn write_str(s: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for &b in s.as_bytes() {
        if b == b'"' || b == b'\\' {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b'"');
}

/// Parses one JSON document (surrounding whitespace allowed).
pub fn parse(text: &[u8]) -> Result<Value, Error> {
    let mut p = Parser { s: text, at: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.at != text.len() {
        return Err(p.err("text after the document"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn err(&self, what: &'static str) -> Error {
        Error { what, at: self.at }
    }
    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.s.get(self.at) {
            self.at += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }
    fn eat(&mut self, lit: &[u8]) -> bool {
        if self.s[self.at..].starts_with(lit) {
            self.at += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deep"));
        }
        self.ws();
        match self.peek() {
            Some(b'{') => {
                self.at += 1;
                let mut m = BTreeMap::new();
                self.ws();
                if self.eat(b"}") {
                    return Ok(Value::Object(m));
                }
                loop {
                    self.ws();
                    if self.peek() != Some(b'"') {
                        return Err(self.err("expected a key"));
                    }
                    let key_at = self.at;
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(b":") {
                        return Err(self.err("expected ':'"));
                    }
                    let v = self.value(depth + 1)?;
                    if m.insert(k, v).is_some() {
                        return Err(Error { what: "duplicate key", at: key_at });
                    }
                    self.ws();
                    if self.eat(b",") {
                        continue;
                    }
                    if self.eat(b"}") {
                        return Ok(Value::Object(m));
                    }
                    return Err(self.err("expected ',' or '}'"));
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut a = Vec::new();
                self.ws();
                if self.eat(b"]") {
                    return Ok(Value::Array(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    if self.eat(b",") {
                        continue;
                    }
                    if self.eat(b"]") {
                        return Ok(Value::Array(a));
                    }
                    return Err(self.err("expected ',' or ']'"));
                }
            }
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') if self.eat(b"true") => Ok(Value::Bool(true)),
            Some(b'f') if self.eat(b"false") => Ok(Value::Bool(false)),
            Some(b'n') if self.eat(b"null") => Ok(Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("not a JSON value")),
            None => Err(self.err("document ends early")),
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.at;
        let negative = self.eat(b"-");
        let digits_at = self.at;
        while let Some(b'0'..=b'9') = self.peek() {
            self.at += 1;
        }
        let digits = &self.s[digits_at..self.at];
        if digits.is_empty() {
            return Err(self.err("expected a digit"));
        }
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(Error { what: "leading zero", at: start });
        }
        if let Some(b'.' | b'e' | b'E') = self.peek() {
            return Err(self.err("not an integer (canonical JSON has only integers)"));
        }
        let mut n: i64 = 0;
        for &d in digits {
            n = n
                .checked_mul(10)
                .and_then(|n| n.checked_add(i64::from(d - b'0')))
                .ok_or(Error { what: "integer out of range", at: start })?;
        }
        Ok(Value::Int(if negative { -n } else { n }))
    }

    fn string(&mut self) -> Result<String, Error> {
        self.at += 1; // the opening quote
        let mut out = Vec::new();
        loop {
            let Some(b) = self.peek() else {
                return Err(self.err("string not closed"));
            };
            self.at += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(e) = self.peek() else {
                        return Err(self.err("string not closed"));
                    };
                    self.at += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode_escape()?,
                        _ => return Err(self.err("unknown escape")),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                _ => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| self.err("string is not UTF-8"))
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let Some(h) = self.s.get(self.at..self.at + 4) else {
            return Err(self.err("short \\u escape"));
        };
        let mut v = 0;
        for &c in h {
            v = v * 16 + (c as char).to_digit(16).ok_or(self.err("bad \\u escape"))?;
        }
        self.at += 4;
        Ok(v)
    }

    fn unicode_escape(&mut self) -> Result<char, Error> {
        let hi = self.hex4()?;
        let code = if (0xd800..0xdc00).contains(&hi) {
            if !self.eat(b"\\u") {
                return Err(self.err("lone surrogate"));
            }
            let lo = self.hex4()?;
            if !(0xdc00..0xe000).contains(&lo) {
                return Err(self.err("lone surrogate"));
            }
            0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
        } else {
            hi
        };
        char::from_u32(code).ok_or(self.err("lone surrogate"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn canonical_sorts_keys_and_drops_whitespace() {
        let v = parse(br#" { "b" : [1, -20, true, null], "a" : "x\"y\\z\n" } "#).unwrap();
        assert_eq!(v.canonical(), b"{\"a\":\"x\\\"y\\\\z\n\",\"b\":[1,-20,true,null]}".to_vec());
        // The canonical form parses back to the same value.
        assert_eq!(parse(&v.canonical()).unwrap(), v);
    }

    #[test]
    fn escapes_decode() {
        let v = parse(br#""\u00e9\ud83d\ude00\/""#).unwrap();
        assert_eq!(v, Value::Str("\u{e9}\u{1f600}/".into()));
    }

    #[test]
    fn refusals() {
        let what = |t: &[u8]| parse(t).unwrap_err().what;
        assert_eq!(what(br#"{"a":1,"a":2}"#), "duplicate key");
        assert_eq!(what(b"1.5"), "not an integer (canonical JSON has only integers)");
        assert_eq!(what(b"1e3"), "not an integer (canonical JSON has only integers)");
        assert_eq!(what(b"01"), "leading zero");
        assert_eq!(what(b"99999999999999999999"), "integer out of range");
        assert_eq!(what(b"[1] x"), "text after the document");
        assert_eq!(what(b"\"\\ud800\""), "lone surrogate");
        assert_eq!(what(b"{\"a\" 1}"), "expected ':'");
        let deep = vec![b'['; MAX_DEPTH + 2];
        assert_eq!(what(&deep), "nested too deep");
    }

    #[test]
    fn integers_round_trip() {
        for n in [0, 7, -7, i64::MAX, i64::MIN + 1] {
            let mut out = Vec::new();
            write_int(n, &mut out);
            assert_eq!(parse(&out).unwrap(), Value::Int(n));
        }
    }
}
