//! The panel's conversation with the model (ROADMAP LY "Change this
//! program", step 4; ARCHITECTURE "The agent panel", "Relay"): the body
//! agent.exe POSTs, and the answer read back out of the reply.
//!
//! The panel speaks the chat-completions wire form (`{"messages":[...]}`)
//! to `HOST`:`PORT` `PATH`: in a scenario that is `cargo xtask
//! agent-relay` behind QEMU's guestfwd, which adds the model's name and
//! the key (the panel holds neither) and records or replays the call.
//! The body is a function of the panel's state alone, so a recording
//! made once replays byte for byte.

use alloc::string::String;
use alloc::vec::Vec;

use crate::handoff::About;
use crate::{Answer, Context, Request};

/// The relay's address on QEMU's user network (xtask's guestfwd).
pub const HOST: &str = "10.0.2.101";
pub const PORT: u16 = 80;
pub const PATH: &str = "/v1/chat/completions";
/// The longest reply read (bytes); a longer one is not answered.
pub const MAX_REPLY: usize = 1 << 20;

/// The request body for the conversation so far: a system message naming
/// the program it is about (and the process handed over, with the text
/// its sender gave for it), then each request asked (hand-offs are not
/// asked yet) and each answer given, in order. With no process the
/// system message is what it was before processes could be handed over,
/// byte for byte (the recordings replay by the body's hash).
pub fn body(context: Option<&Context>, about: Option<&About>, requests: &[Request]) -> String {
    let mut s = String::from("{\"messages\":[");
    let mut system = match context {
        Some(c) => alloc::format!(
            "You are the agent in gala's side panel. The user is working in {} \
             (window \"{}\"). Answer briefly.",
            c.program,
            c.title
        ),
        None => String::from("You are the agent in gala's side panel. Answer briefly."),
    };
    if let Some(a) = about {
        system.push_str(&alloc::format!(
            " The user asks about the process {} (pid {}, {}), as {} shows it:\n{}",
            a.name,
            a.pid,
            a.personality(),
            a.program,
            a.text
        ));
    }
    message(&mut s, "system", &system);
    for r in requests {
        let Some(a) = &r.answer else { continue };
        s.push(',');
        message(&mut s, "user", &r.text);
        if let Answer::Said(t) = a {
            s.push(',');
            message(&mut s, "assistant", t);
        }
    }
    s.push_str("]}");
    s
}

fn message(s: &mut String, role: &str, content: &str) {
    s.push_str("{\"role\":\"");
    s.push_str(role);
    s.push_str("\",\"content\":");
    quote(s, content);
    s.push('}');
}

/// `t` as a JSON string.
fn quote(s: &mut String, t: &str) {
    s.push('"');
    for c in t.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => s.push_str(&alloc::format!("\\u{:04x}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
}

/// What a reply says: `choices[0].message.content`, or why there is no
/// answer (the reply's `error.message`, or what it lacks).
pub fn answer(reply: &[u8]) -> Result<String, String> {
    let text = core::str::from_utf8(reply).map_err(|_| String::from("the reply is not UTF-8"))?;
    let v = Parser { s: text.as_bytes(), at: 0 }
        .document()
        .ok_or_else(|| String::from("the reply is not JSON"))?;
    if let Some(e) = v.get("error") {
        return Err(match e.get("message") {
            Some(Value::Str(m)) => m.clone(),
            _ => String::from("an error with no message"),
        });
    }
    let content = v
        .get("choices")
        .and_then(|c| c.index(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"));
    match content {
        Some(Value::Str(t)) => Ok(t.clone()),
        _ => Err(String::from("no choices[0].message.content in the reply")),
    }
}

/// A JSON value, as far as reading a reply needs (numbers are skipped).
#[derive(Debug, PartialEq)]
enum Value {
    Null,
    Bool(bool),
    Number,
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(o) => o.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn index(&self, i: usize) -> Option<&Value> {
        match self {
            Value::Array(a) => a.get(i),
            _ => None,
        }
    }
}

/// How deep arrays and objects may nest.
const DEPTH: usize = 64;

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn document(mut self) -> Option<Value> {
        let v = self.value(0)?;
        self.space();
        (self.at == self.s.len()).then_some(v)
    }

    fn space(&mut self) {
        while self.s.get(self.at).is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.space();
        let ok = self.s.get(self.at) == Some(&c);
        if ok {
            self.at += 1;
        }
        ok
    }

    fn word(&mut self, w: &[u8]) -> bool {
        let ok = self.s[self.at..].starts_with(w);
        if ok {
            self.at += w.len();
        }
        ok
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > DEPTH {
            return None;
        }
        self.space();
        match *self.s.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut o = Vec::new();
                if self.eat(b'}') {
                    return Some(Value::Object(o));
                }
                loop {
                    self.space();
                    let k = self.string()?;
                    if !self.eat(b':') {
                        return None;
                    }
                    o.push((k, self.value(depth + 1)?));
                    if self.eat(b'}') {
                        return Some(Value::Object(o));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut a = Vec::new();
                if self.eat(b']') {
                    return Some(Value::Array(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    if self.eat(b']') {
                        return Some(Value::Array(a));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'"' => self.string().map(Value::Str),
            b't' => self.word(b"true").then_some(Value::Bool(true)),
            b'f' => self.word(b"false").then_some(Value::Bool(false)),
            b'n' => self.word(b"null").then_some(Value::Null),
            b'-' | b'0'..=b'9' => {
                let start = self.at;
                while self
                    .s
                    .get(self.at)
                    .is_some_and(|c| matches!(c, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
                {
                    self.at += 1;
                }
                (self.at > start).then_some(Value::Number)
            }
            _ => None,
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.s.get(self.at..self.at + 4)?;
        let v = u32::from_str_radix(core::str::from_utf8(h).ok()?, 16).ok()?;
        self.at += 4;
        Some(v)
    }

    fn string(&mut self) -> Option<String> {
        if self.s.get(self.at) != Some(&b'"') {
            return None;
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            // The run up to the next quote or escape, as it is (the
            // input is UTF-8 already; both are ASCII).
            let start = self.at;
            while self.s.get(self.at).is_some_and(|&c| c != b'"' && c != b'\\' && c >= 0x20) {
                self.at += 1;
            }
            out.push_str(core::str::from_utf8(&self.s[start..self.at]).ok()?);
            match *self.s.get(self.at)? {
                b'"' => {
                    self.at += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.at += 1;
                    let e = *self.s.get(self.at)?;
                    self.at += 1;
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
                            let mut c = self.hex4()?;
                            if (0xd800..0xdc00).contains(&c) {
                                if !self.word(b"\\u") {
                                    return None;
                                }
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return None;
                                }
                                c = 0x10000 + ((c - 0xd800) << 10) + (lo - 0xdc00);
                            }
                            out.push(char::from_u32(c)?);
                        }
                        _ => return None,
                    }
                }
                // A raw control character inside a string.
                _ => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(text: &str, answer: Answer) -> Request {
        Request {
            program: "Notepad".into(),
            text: text.into(),
            element: None,
            answer: Some(answer),
        }
    }

    #[test]
    fn the_body_is_the_conversation() {
        let c = Context { program: "Notepad".into(), title: "a \"b\"".into() };
        let rs = [
            asked("hi\n", Answer::Said("hello".into())),
            Request { program: "x".into(), text: "handed".into(), element: None, answer: None },
            asked("again", Answer::Asking),
        ];
        assert_eq!(
            body(Some(&c), None, &rs),
            "{\"messages\":[{\"role\":\"system\",\"content\":\"You are the agent in gala's \
             side panel. The user is working in Notepad (window \\\"a \\\"b\\\"\\\"). \
             Answer briefly.\"},{\"role\":\"user\",\"content\":\"hi\\n\"},{\"role\":\
             \"assistant\",\"content\":\"hello\"},{\"role\":\"user\",\"content\":\"again\"}]}"
        );
        // A process handed over: its text follows the system message's.
        let a = About {
            program: "Task Manager".into(),
            title: "Task Manager".into(),
            window: 0x20010,
            name: "busy.exe".into(),
            pid: 52,
            linux: false,
            command_line: "busy.exe".into(),
            text: "CPU: 50.0%".into(),
        };
        assert_eq!(
            body(None, Some(&a), &[]),
            "{\"messages\":[{\"role\":\"system\",\"content\":\"You are the agent in gala's \
             side panel. Answer briefly. The user asks about the process busy.exe (pid 52, \
             Windows), as Task Manager shows it:\\nCPU: 50.0%\"}]}"
        );
        // Control characters are escaped.
        let mut s = String::new();
        quote(&mut s, "\u{1}\t");
        assert_eq!(s, "\"\\u0001\\t\"");
    }

    #[test]
    fn the_answer_is_read_out() {
        let reply = br#"{"id":"x","choices":[{"index":0,"message":{"role":"assistant",
            "content":"Pong! \u00e9\ud83d\ude00 \"q\"\n","refusal":null},"logprobs":null,
            "finish_reason":"stop"}],"usage":{"total_tokens":12.5e1,"ok":true}}"#;
        assert_eq!(answer(reply), Ok("Pong! \u{e9}\u{1f600} \"q\"\n".into()));
        assert_eq!(
            answer(br#"{"error":{"message":"agent-relay: not recorded","type":null}}"#),
            Err("agent-relay: not recorded".into())
        );
        assert_eq!(
            answer(br#"{"choices":[]}"#),
            Err("no choices[0].message.content in the reply".into())
        );
        assert_eq!(answer(b"{\"a\":1"), Err("the reply is not JSON".into()));
        assert_eq!(answer(b"{} x"), Err("the reply is not JSON".into()));
        assert_eq!(answer(&[0xff]), Err("the reply is not UTF-8".into()));
        assert_eq!(answer(b"\"\\ud800x\""), Err("the reply is not JSON".into()));
        let deep = [b'['; DEPTH + 2];
        assert_eq!(answer(&deep), Err("the reply is not JSON".into()));
    }
}
