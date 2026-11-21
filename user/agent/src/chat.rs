//! The panel's conversation with the model (ROADMAP LY "Change this
//! program", step 4; ARCHITECTURE "The agent panel", "Relay" and
//! "Tools"): the body agent.exe POSTs, with the one tool it offers
//! (`run`), and what the reply says: text, or calls of the tool.
//!
//! The panel speaks the chat-completions wire form (`{"messages":[...],
//! "tools":[...]}`) to `HOST`:`PORT` `PATH`: in a scenario that is
//! `cargo xtask agent-relay` behind QEMU's guestfwd, which adds the
//! model's name and the key (the panel holds neither) and records or
//! replays the call. The body is a function of the panel's state alone,
//! so a recording made once replays byte for byte.

use alloc::string::String;
use alloc::vec::Vec;

use crate::handoff::About;
use crate::{Answer, Call, Context, Ran, Request, Step, Tools, Work};

/// The relay's address on QEMU's user network (xtask's guestfwd).
pub const HOST: &str = "10.0.2.101";
pub const PORT: u16 = 80;
pub const PATH: &str = "/v1/chat/completions";
/// The longest reply read (bytes); a longer one is not answered.
pub const MAX_REPLY: usize = 1 << 20;
/// The most of a command's output (standard output and error) kept and
/// given back to the model (bytes); the rest is read and counted.
pub const MAX_OUTPUT: usize = 16 * 1024;
/// The tool's name.
pub const RUN: &str = "run";

/// The request body for the conversation so far: a system message naming
/// the program it is about (and the process handed over, with the text
/// its sender gave for it), then each request asked (hand-offs are not
/// asked yet) with its rounds (the model's calls of `run` and their
/// results) and its answer, in order; then the tool, described for the
/// working folder of the request being asked (`run_description`).
pub fn body(
    context: Option<&Context>,
    about: Option<&About>,
    requests: &[Request],
    tools: &Tools,
) -> String {
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
    message(&mut s, "system", Some(&system));
    let mut work = &Work::Unknown(String::from("nothing was asked yet"));
    for r in requests {
        let Some(a) = &r.answer else { continue };
        work = &r.work;
        s.push(',');
        message(&mut s, "user", Some(&r.text));
        for round in &r.rounds {
            // A round goes to the model once each of its calls has a
            // result (the API wants one for every call).
            let results: Option<Vec<String>> = round.calls.iter().map(result).collect();
            let Some(results) = results else { continue };
            s.push_str(",{\"role\":\"assistant\",\"content\":");
            match &round.text {
                Some(t) => quote(&mut s, t),
                None => s.push_str("null"),
            }
            s.push_str(",\"tool_calls\":[");
            for (k, c) in round.calls.iter().enumerate() {
                if k > 0 {
                    s.push(',');
                }
                s.push_str("{\"id\":");
                quote(&mut s, &c.id);
                s.push_str(",\"type\":\"function\",\"function\":{\"name\":");
                quote(&mut s, &c.name);
                s.push_str(",\"arguments\":");
                quote(&mut s, &c.arguments);
                s.push_str("}}");
            }
            s.push_str("]}");
            for (c, text) in round.calls.iter().zip(&results) {
                s.push_str(",{\"role\":\"tool\",\"tool_call_id\":");
                quote(&mut s, &c.id);
                s.push_str(",\"content\":");
                quote(&mut s, text);
                s.push('}');
            }
        }
        if let Answer::Said(t) = a {
            s.push(',');
            message(&mut s, "assistant", Some(t));
        }
    }
    s.push_str(
        "],\"tools\":[{\"type\":\"function\",\"function\":{\"name\":\"run\",\"description\":",
    );
    quote(&mut s, &run_description(work, tools));
    s.push_str(
        ",\"parameters\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\",\
         \"description\":\"The command line to run.\"},\"explanation\":{\"type\":\"string\",\
         \"description\":\"One sentence for the user: what the command does and why.\"}},\
         \"required\":[\"command\",\"explanation\"],\"additionalProperties\":false}}}],\
         \"parallel_tool_calls\":false}",
    );
    s
}

/// What `run` is, for the working folder `work` on this machine: where
/// and how a command runs, or why every run is refused.
pub fn run_description(work: &Work, tools: &Tools) -> String {
    let mut d = alloc::format!(
        "Runs a command line for the user and gives back its exit code and its output \
         (standard output and error, at most {MAX_OUTPUT} bytes). Nothing runs until the \
         user, shown the command and your explanation, clicks Run; they may cancel it."
    );
    match work {
        Work::Windows { program, folder } => match &tools.cmd {
            Some(_) => {
                d.push_str(&alloc::format!(
                    " It runs as `cmd.exe /c <command>` in the folder {program} shows: \
                     {folder}. gala's cmd.exe runs one program per command line: no &, &&, \
                     ||, | or batch files."
                ));
                if tools.busybox {
                    d.push_str(
                        " There is no tar.exe; busybox (busybox-w32, on the PATH) has tar \
                         and gzip: `busybox tar -czf <name>.tar.gz <files>` makes a \
                         compressed archive.",
                    );
                }
            }
            None => d.push_str(&alloc::format!(
                " The folder {program} shows is {folder}, but gala's cmd.exe is not \
                 installed: every run is refused."
            )),
        },
        Work::Linux { program, distro, path } => d.push_str(&alloc::format!(
            " The folder {program} shows is {path} in the Linux distribution {distro}, \
             where running commands is not supported yet: every run is refused."
        )),
        Work::Unknown(why) => d.push_str(&alloc::format!(
            " No working folder is known ({why}): every run is refused."
        )),
    }
    d
}

/// The tool message for call `c`, once it has one.
pub fn result(c: &Call) -> Option<String> {
    Some(match &c.step {
        Step::Waiting | Step::Running => return None,
        Step::Declined => String::from("The user declined to run this command."),
        Step::Refused(why) => alloc::format!("Not run: {why}."),
        Step::Ran(Ran { code: None, output, .. }) => alloc::format!("Could not run it: {output}."),
        Step::Ran(r) => {
            let mut t = alloc::format!("Exit code {}.", r.code.unwrap_or_default());
            if r.bytes == 0 {
                t.push_str(" No output.");
            } else if r.bytes > r.output.len() {
                t.push_str(&alloc::format!(
                    " Output (the first {} of {} bytes):\n{}",
                    r.output.len(),
                    r.bytes,
                    r.output
                ));
            } else {
                t.push_str(&alloc::format!(" Output:\n{}", r.output));
            }
            t
        }
    })
}

fn message(s: &mut String, role: &str, content: Option<&str>) {
    s.push_str("{\"role\":\"");
    s.push_str(role);
    s.push_str("\",\"content\":");
    match content {
        Some(c) => quote(s, c),
        None => s.push_str("null"),
    }
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

/// A call of a tool in a reply: its id, the tool's name and its
/// arguments as the model wrote them (a JSON object in a string, given
/// back as they are in the next request).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// What the model said: a final text, or calls of a tool (with any text
/// it said beside them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    Text(String),
    Calls { text: Option<String>, calls: Vec<ToolCall> },
}

/// What a reply says: `choices[0].message`'s `tool_calls` when it has
/// any, else its `content`; or why there is no answer (the reply's
/// `error.message`, or what it lacks).
pub fn reply(bytes: &[u8]) -> Result<Said, String> {
    let text = core::str::from_utf8(bytes).map_err(|_| String::from("the reply is not UTF-8"))?;
    let v = Parser { s: text.as_bytes(), at: 0 }
        .document()
        .ok_or_else(|| String::from("the reply is not JSON"))?;
    if let Some(e) = v.get("error") {
        return Err(match e.get("message") {
            Some(Value::Str(m)) => m.clone(),
            _ => String::from("an error with no message"),
        });
    }
    let m = v
        .get("choices")
        .and_then(|c| c.index(0))
        .and_then(|c| c.get("message"))
        .ok_or_else(|| String::from("no choices[0].message in the reply"))?;
    let content = match m.get("content") {
        Some(Value::Str(t)) => Some(t.clone()),
        None | Some(Value::Null) => None,
        Some(_) => return Err(String::from("choices[0].message.content is not a string")),
    };
    let calls = match m.get("tool_calls") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => {
            let mut calls = Vec::new();
            for (k, c) in a.iter().enumerate() {
                let f = c.get("function");
                let field = |v: Option<&Value>, what: &str| match v {
                    Some(Value::Str(s)) => Ok(s.clone()),
                    _ => Err(alloc::format!("tool call {k} has no {what}")),
                };
                calls.push(ToolCall {
                    id: field(c.get("id"), "id")?,
                    name: field(f.and_then(|f| f.get("name")), "function.name")?,
                    arguments: field(f.and_then(|f| f.get("arguments")), "function.arguments")?,
                });
            }
            calls
        }
        Some(_) => return Err(String::from("choices[0].message.tool_calls is not an array")),
    };
    match (content, calls.is_empty()) {
        (Some(t), true) => Ok(Said::Text(t)),
        (None, true) => Err(String::from("no content and no tool calls in choices[0].message")),
        (text, false) => Ok(Said::Calls { text, calls }),
    }
}

/// `run`'s arguments: the command line and the explanation (the
/// arguments are JSON in a string; the command must be a non-empty
/// string, the explanation a string, empty when missing).
pub fn run_arguments(arguments: &str) -> Result<(String, String), String> {
    let v = Parser { s: arguments.as_bytes(), at: 0 }
        .document()
        .ok_or_else(|| String::from("its arguments are not JSON"))?;
    if !matches!(v, Value::Object(_)) {
        return Err(String::from("its arguments are not a JSON object"));
    }
    let command = match v.get("command") {
        Some(Value::Str(c)) if !c.trim().is_empty() => c.clone(),
        Some(Value::Str(_)) => return Err(String::from("its command is empty")),
        _ => return Err(String::from("it has no command string")),
    };
    let explanation = match v.get("explanation") {
        Some(Value::Str(e)) => e.clone(),
        None | Some(Value::Null) => String::new(),
        Some(_) => return Err(String::from("its explanation is not a string")),
    };
    Ok((command, explanation))
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
    use crate::Round;

    fn asked(text: &str, answer: Answer) -> Request {
        Request {
            program: "Notepad".into(),
            text: text.into(),
            element: None,
            answer: Some(answer),
            work: Work::Unknown("Notepad reports no folder".into()),
            rounds: Vec::new(),
            asked: 1,
        }
    }

    fn tools() -> Tools {
        Tools { cmd: Some("C:\\Windows\\System32\\cmd.exe".into()), busybox: true }
    }

    /// The tool as it follows the messages, described for `work`.
    fn tool(work: &Work) -> String {
        let mut s = String::from(
            "],\"tools\":[{\"type\":\"function\",\"function\":{\"name\":\"run\",\"description\":",
        );
        quote(&mut s, &run_description(work, &tools()));
        s.push_str(
            ",\"parameters\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\
             \"string\",\"description\":\"The command line to run.\"},\"explanation\":{\
             \"type\":\"string\",\"description\":\"One sentence for the user: what the \
             command does and why.\"}},\"required\":[\"command\",\"explanation\"],\
             \"additionalProperties\":false}}}],\"parallel_tool_calls\":false}",
        );
        s
    }

    #[test]
    fn the_body_is_the_conversation() {
        let c = Context { program: "Notepad".into(), title: "a \"b\"".into(), window: 0 };
        let handed = Request { answer: None, text: "handed".into(), ..asked("", Answer::Asking) };
        let rs =
            [asked("hi\n", Answer::Said("hello".into())), handed, asked("again", Answer::Asking)];
        let work = Work::Unknown("Notepad reports no folder".into());
        assert_eq!(
            body(Some(&c), None, &rs, &tools()),
            alloc::format!(
                "{{\"messages\":[{{\"role\":\"system\",\"content\":\"You are the agent in \
                 gala's side panel. The user is working in Notepad (window \\\"a \\\"b\\\"\\\"). \
                 Answer briefly.\"}},{{\"role\":\"user\",\"content\":\"hi\\n\"}},{{\"role\":\
                 \"assistant\",\"content\":\"hello\"}},{{\"role\":\"user\",\"content\":\
                 \"again\"}}{}",
                tool(&work)
            )
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
            body(None, Some(&a), &[], &tools()),
            alloc::format!(
                "{{\"messages\":[{{\"role\":\"system\",\"content\":\"You are the agent in \
                 gala's side panel. Answer briefly. The user asks about the process busy.exe \
                 (pid 52, Windows), as Task Manager shows it:\\nCPU: 50.0%\"}}{}",
                tool(&Work::Unknown("nothing was asked yet".into()))
            )
        );
        // Control characters are escaped.
        let mut s = String::new();
        quote(&mut s, "\u{1}\t");
        assert_eq!(s, "\"\\u0001\\t\"");
    }

    fn call(id: &str, arguments: &str, step: Step) -> Call {
        Call {
            id: id.into(),
            name: RUN.into(),
            arguments: arguments.into(),
            command: String::new(),
            explanation: String::new(),
            step,
        }
    }

    #[test]
    fn the_rounds_go_back_with_their_results() {
        let work = Work::Windows {
            program: "File Explorer".into(),
            folder: "C:\\Users\\gala\\AppData\\Local\\Temp\\agent_run".into(),
        };
        let mut r =
            Request { work: work.clone(), ..asked("compress all the files", Answer::Asking) };
        let ran = |code, output: &str, bytes| Step::Ran(Ran { code, output: output.into(), bytes });
        r.rounds = [
            Round {
                text: None,
                calls: [call("c1", "{\"command\":\"dir\"}", ran(Some(0), "a.txt\r\n", 7))].into(),
            },
            Round {
                text: Some("Next:".into()),
                calls: [
                    call("c2", "{}", Step::Refused("it has no command string".into())),
                    call("c3", "{\"command\":\"del x\"}", Step::Declined),
                    call("c4", "{\"command\":\"big\"}", ran(Some(1), "xy", 9)),
                    call("c5", "{\"command\":\"none\"}", ran(None, "the folder is gone", 0)),
                    call("c6", "{\"command\":\"quiet\"}", ran(Some(0), "", 0)),
                ]
                .into(),
            },
            // Not yet run: not sent.
            Round { text: None, calls: [call("c7", "{}", Step::Waiting)].into() },
        ]
        .into();
        let b = body(None, None, &[r], &tools());
        let messages = "{\"role\":\"user\",\"content\":\"compress all the files\"},\
             {\"role\":\"assistant\",\"content\":null,\"tool_calls\":[{\"id\":\"c1\",\"type\":\
             \"function\",\"function\":{\"name\":\"run\",\"arguments\":\"{\\\"command\\\":\
             \\\"dir\\\"}\"}}]},{\"role\":\"tool\",\"tool_call_id\":\"c1\",\"content\":\
             \"Exit code 0. Output:\\na.txt\\r\\n\"},{\"role\":\"assistant\",\"content\":\
             \"Next:\",\"tool_calls\":[";
        assert!(b.contains(messages), "{b}");
        for result in [
            "{\"role\":\"tool\",\"tool_call_id\":\"c2\",\"content\":\"Not run: it has no \
             command string.\"}",
            "{\"role\":\"tool\",\"tool_call_id\":\"c3\",\"content\":\"The user declined to run \
             this command.\"}",
            "{\"role\":\"tool\",\"tool_call_id\":\"c4\",\"content\":\"Exit code 1. Output (the \
             first 2 of 9 bytes):\\nxy\"}",
            "{\"role\":\"tool\",\"tool_call_id\":\"c5\",\"content\":\"Could not run it: the \
             folder is gone.\"}",
            "{\"role\":\"tool\",\"tool_call_id\":\"c6\",\"content\":\"Exit code 0. No output.\"}",
        ] {
            assert!(b.contains(result), "{result} in {b}");
        }
        assert!(!b.contains("c7"), "{b}");
        assert!(b.ends_with(&tool(&work)), "{b}");
        let d = run_description(&work, &tools());
        assert!(d.contains("in the folder File Explorer shows: C:\\Users\\gala\\AppData"), "{d}");
        assert!(d.contains("`busybox tar -czf <name>.tar.gz <files>`"), "{d}");
        let bare = run_description(&work, &Tools { cmd: Some("cmd".into()), busybox: false });
        assert!(!bare.contains("busybox"), "{bare}");
        let none = run_description(&work, &Tools::default());
        assert!(none.ends_with("but gala's cmd.exe is not installed: every run is refused."));
        let linux = Work::Linux {
            program: "File Explorer".into(),
            distro: "Alpine".into(),
            path: "/home".into(),
        };
        assert!(run_description(&linux, &tools()).ends_with(
            "The folder File Explorer shows is /home in the Linux distribution Alpine, where \
             running commands is not supported yet: every run is refused."
        ));
        assert!(
            run_description(&Work::Unknown("no window is active".into()), &tools()).ends_with(
                " No working folder is known (no window is active): every run is refused."
            )
        );
    }

    #[test]
    fn the_answer_is_read_out() {
        let r = br#"{"id":"x","choices":[{"index":0,"message":{"role":"assistant",
            "content":"Pong! \u00e9\ud83d\ude00 \"q\"\n","refusal":null},"logprobs":null,
            "finish_reason":"stop"}],"usage":{"total_tokens":12.5e1,"ok":true}}"#;
        assert_eq!(reply(r), Ok(Said::Text("Pong! \u{e9}\u{1f600} \"q\"\n".into())));
        assert_eq!(
            reply(br#"{"error":{"message":"agent-relay: not recorded","type":null}}"#),
            Err("agent-relay: not recorded".into())
        );
        assert_eq!(reply(br#"{"choices":[]}"#), Err("no choices[0].message in the reply".into()));
        assert_eq!(
            reply(br#"{"choices":[{"message":{"content":null}}]}"#),
            Err("no content and no tool calls in choices[0].message".into())
        );
        assert_eq!(reply(b"{\"a\":1"), Err("the reply is not JSON".into()));
        assert_eq!(reply(b"{} x"), Err("the reply is not JSON".into()));
        assert_eq!(reply(&[0xff]), Err("the reply is not UTF-8".into()));
        assert_eq!(reply(b"\"\\ud800x\""), Err("the reply is not JSON".into()));
        let deep = [b'['; DEPTH + 2];
        assert_eq!(reply(&deep), Err("the reply is not JSON".into()));
    }

    #[test]
    fn tool_calls_are_read_out() {
        let r = br#"{"choices":[{"index":0,"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"call_1","type":"function","function":{"name":"run",
            "arguments":"{\"command\":\"busybox tar -czf files.tar.gz one.txt\",\"explanation\":\"Packs them.\"}"}}],
            "refusal":null},"finish_reason":"tool_calls"}]}"#;
        let arguments = "{\"command\":\"busybox tar -czf files.tar.gz one.txt\",\"explanation\":\"Packs them.\"}";
        assert_eq!(
            reply(r),
            Ok(Said::Calls {
                text: None,
                calls: [ToolCall {
                    id: "call_1".into(),
                    name: "run".into(),
                    arguments: arguments.into()
                }]
                .into(),
            })
        );
        assert_eq!(
            run_arguments(arguments),
            Ok(("busybox tar -czf files.tar.gz one.txt".into(), "Packs them.".into()))
        );
        // Text beside the calls is kept.
        let both = br#"{"choices":[{"message":{"content":"First:","tool_calls":[
            {"id":"a","function":{"name":"x","arguments":"{}"}}]}}]}"#;
        assert_eq!(
            reply(both),
            Ok(Said::Calls {
                text: Some("First:".into()),
                calls: [ToolCall { id: "a".into(), name: "x".into(), arguments: "{}".into() }]
                    .into(),
            })
        );
        // A call missing its parts is no reply.
        assert_eq!(
            reply(br#"{"choices":[{"message":{"tool_calls":[{"function":{"name":"run","arguments":"{}"}}]}}]}"#),
            Err("tool call 0 has no id".into())
        );
        assert_eq!(
            reply(br#"{"choices":[{"message":{"tool_calls":[{"id":"a","function":{"name":"run","arguments":{}}}]}}]}"#),
            Err("tool call 0 has no function.arguments".into())
        );
        assert_eq!(
            reply(br#"{"choices":[{"message":{"tool_calls":{}}}]}"#),
            Err("choices[0].message.tool_calls is not an array".into())
        );
        // Arguments that are not run's.
        assert_eq!(run_arguments("{\"command\""), Err("its arguments are not JSON".into()));
        assert_eq!(run_arguments("[]"), Err("its arguments are not a JSON object".into()));
        assert_eq!(run_arguments("{}"), Err("it has no command string".into()));
        assert_eq!(run_arguments("{\"command\":3}"), Err("it has no command string".into()));
        assert_eq!(run_arguments("{\"command\":\" \"}"), Err("its command is empty".into()));
        assert_eq!(run_arguments("{\"command\":\"dir\"}"), Ok(("dir".into(), String::new())));
        assert_eq!(
            run_arguments("{\"command\":\"dir\",\"explanation\":1}"),
            Err("its explanation is not a string".into())
        );
    }
}
