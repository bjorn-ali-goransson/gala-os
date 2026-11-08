//! Generates the interfaces' request and event tables (`protocol.rs` in
//! OUT_DIR) from the protocol XML in `protocol/`: for each interface its
//! name and version, and for each request and event its name, the
//! version that added it (`since`) and its arguments (wire type, the
//! interface of an object or new_id, whether null is allowed). Enums and
//! descriptions are not needed by the wire format and are skipped.
//!
//! The XML is the Wayland project's (MIT, THIRD_PARTY.md), read with a
//! small scanner of our own: the files use elements, attributes in double
//! quotes, comments and text, nothing else.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros, clippy::disallowed_types)]

use std::fmt::Write as _;
use std::path::Path;

const FILES: &[&str] = &["wayland.xml", "xdg-shell.xml", "xdg-decoration-unstable-v1.xml"];

struct Arg {
    name: String,
    kind: String,
    interface: Option<String>,
    nullable: bool,
}

struct Message {
    name: String,
    since: u32,
    destructor: bool,
    args: Vec<Arg>,
}

struct Interface {
    name: String,
    version: u32,
    requests: Vec<Message>,
    events: Vec<Message>,
}

/// One tag: its name, whether it closes (`</x>`), whether it is
/// self-closing (`<x/>`), and its attributes.
struct Tag {
    name: String,
    closing: bool,
    empty: bool,
    attrs: Vec<(String, String)>,
}

impl Tag {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// The tags of `xml` in order (comments, processing instructions and text
/// skipped).
fn tags(xml: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        if let Some(body) = rest.strip_prefix("<!--") {
            let end = body.find("-->").expect("unterminated comment");
            rest = &body[end + 3..];
            continue;
        }
        if rest.starts_with("<?") {
            let end = rest.find("?>").expect("unterminated processing instruction");
            rest = &rest[end + 2..];
            continue;
        }
        // Quoted attribute values may hold '>', so scan for the end outside
        // quotes.
        let bytes = rest.as_bytes();
        let mut i = 1;
        let mut quoted = false;
        while i < bytes.len() && (quoted || bytes[i] != b'>') {
            if bytes[i] == b'"' {
                quoted = !quoted;
            }
            i += 1;
        }
        let inner = &rest[1..i];
        rest = &rest[i + 1..];
        let closing = inner.starts_with('/');
        let empty = inner.ends_with('/');
        let inner = inner.trim_start_matches('/').trim_end_matches('/').trim();
        let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
        let name = inner[..name_end].to_string();
        let mut attrs = Vec::new();
        let mut a = inner[name_end..].trim();
        while let Some(eq) = a.find('=') {
            let key = a[..eq].trim().to_string();
            let after = a[eq + 1..].trim_start();
            let after = after.strip_prefix('"').expect("attribute value in double quotes");
            let close = after.find('"').expect("unterminated attribute value");
            attrs.push((key, after[..close].to_string()));
            a = after[close + 1..].trim();
        }
        out.push(Tag { name, closing, empty, attrs });
    }
    out
}

fn parse(xml: &str) -> Vec<Interface> {
    let mut interfaces: Vec<Interface> = Vec::new();
    // Which list the current message goes in (true: events).
    let mut in_event = false;
    let mut in_message = false;
    for t in tags(xml) {
        match (t.name.as_str(), t.closing) {
            ("interface", false) => interfaces.push(Interface {
                name: t.attr("name").expect("interface name").to_string(),
                version: t.attr("version").expect("interface version").parse().expect("version"),
                requests: Vec::new(),
                events: Vec::new(),
            }),
            ("request" | "event", false) => {
                in_event = t.name == "event";
                in_message = !t.empty;
                let m = Message {
                    name: t.attr("name").expect("message name").to_string(),
                    since: t.attr("since").map_or(1, |s| s.parse().expect("since")),
                    destructor: t.attr("type") == Some("destructor"),
                    args: Vec::new(),
                };
                let i = interfaces.last_mut().expect("message outside an interface");
                if in_event { i.events.push(m) } else { i.requests.push(m) }
            }
            ("request" | "event", true) => in_message = false,
            ("arg", false) if in_message => {
                let i = interfaces.last_mut().expect("arg outside an interface");
                let list = if in_event { &mut i.events } else { &mut i.requests };
                list.last_mut().expect("arg outside a message").args.push(Arg {
                    name: t.attr("name").expect("arg name").to_string(),
                    kind: t.attr("type").expect("arg type").to_string(),
                    interface: t.attr("interface").map(str::to_string),
                    nullable: t.attr("allow-null") == Some("true"),
                });
            }
            _ => {}
        }
    }
    interfaces
}

fn kind(k: &str) -> &'static str {
    match k {
        "int" => "Int",
        "uint" => "Uint",
        "fixed" => "Fixed",
        "string" => "String",
        "object" => "Object",
        "new_id" => "NewId",
        "array" => "Array",
        "fd" => "Fd",
        other => panic!("unknown argument type {other}"),
    }
}

fn messages(out: &mut String, list: &[Message]) {
    out.push_str("&[");
    for m in list {
        write!(
            out,
            "Message {{ name: {:?}, since: {}, destructor: {}, args: &[",
            m.name, m.since, m.destructor
        )
        .unwrap();
        for a in &m.args {
            write!(
                out,
                "ArgSpec {{ name: {:?}, kind: ArgKind::{}, interface: {:?}, nullable: {} }},",
                a.name,
                kind(&a.kind),
                a.interface,
                a.nullable
            )
            .unwrap();
        }
        out.push_str("] },");
    }
    out.push(']');
}

fn main() {
    let dir = Path::new("protocol");
    let mut all = Vec::new();
    for f in FILES {
        let path = dir.join(f);
        println!("cargo:rerun-if-changed={}", path.display());
        let xml = std::fs::read_to_string(&path).expect("protocol XML");
        all.extend(parse(&xml));
    }
    let mut out = String::from(
        "/// Every interface of the protocol files, in file order.\npub static INTERFACES: &[Interface] = &[\n",
    );
    for i in &all {
        write!(out, "Interface {{ name: {:?}, version: {}, requests: ", i.name, i.version).unwrap();
        messages(&mut out, &i.requests);
        out.push_str(", events: ");
        messages(&mut out, &i.events);
        out.push_str(" },\n");
    }
    out.push_str("];\n");
    let most = all.iter().flat_map(|i| i.requests.iter().chain(&i.events)).map(|m| m.args.len());
    writeln!(
        out,
        "/// The most arguments any request or event has.\npub const MAX_ARGS: usize = {};",
        most.max().unwrap_or(0)
    )
    .unwrap();
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("protocol.rs");
    std::fs::write(dest, out).expect("writing protocol.rs");
}
