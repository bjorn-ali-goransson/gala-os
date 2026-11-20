//! The agent panel's state, view and update (ROADMAP LY "Change this
//! program", step 4; ARCHITECTURE "The agent panel"): what agent.exe
//! shows, as a gala_ui component, without system calls (host tests
//! below).
//!
//! - **Docked** (`Panel::hidden` false): the whole panel, `WIDTH` wide
//!   and the work area's height: a header with Hide, the context chip
//!   naming the active window's program and title, the requests made so
//!   far, and a prompt to type one into.
//! - **Hidden**: only the dock tab ("DockTab"), `TAB` in size; a click
//!   on it docks the panel again.
//!
//! - **Hand-off** ([`handoff`]): a request another program made for
//!   itself (Explorer's "Make the change"), received by WM_COPYDATA; the
//!   panel docks if hidden, takes the program as its context and lists
//!   the request with the part of the window it is about.
//!
//! - **Chat** ([`chat`]): a request typed and sent is asked of the model
//!   (the program POSTs `chat::body` on a worker thread); its reply card
//!   ("Reply{i}") says it is being asked, then the answer, or why there
//!   is none.
//!
//! The program (main.rs) does what a message means outside the view:
//! the app bar's strip, the window's place, the remembered state, the
//! call to the model.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod chat;

use alloc::string::String;
use alloc::vec::Vec;

use gala_ui::{Align, Node, button, column, input, row, text};

/// The docked panel's width (its app bar strip).
pub const WIDTH: i32 = 320;
/// The dock tab's size while hidden, at the right edge of the screen.
pub const TAB: (i32, i32) = (56, 40);

const SURFACE: u32 = 0xf3f3f3;
const CARD: u32 = 0xffffff;
const LINE: u32 = 0xe0e0e0;
const INK: u32 = 0x1b1b1b;
const MUTED: u32 = 0x616161;
const ACCENT: u32 = 0x005fb8;
const CHIP: u32 = 0xe6eef8;
const FAILED: u32 = 0xc42b1c;

/// What the panel is about: the active window's program and title.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Context {
    /// The program's name ("File Explorer", else its file's name).
    pub program: String,
    /// The window's title.
    pub title: String,
}

/// A request the user made, and the program it is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub program: String,
    pub text: String,
    /// The part of the program's window it is about (a hand-off's
    /// picked element: its label), if any.
    pub element: Option<String>,
    /// What the model said, for a request asked of it (None: a hand-off,
    /// listed and not asked yet).
    pub answer: Option<Answer>,
}

/// A request's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Asked, no reply yet.
    Asking,
    Said(String),
    /// Why there is no answer (the relay's or the network's error).
    Failed(String),
}

/// What asking the model gave: the answer, or why there is none.
pub type Reply = Result<String, String>;

/// WM_COPYDATA's hand-off of a request to the panel (ROADMAP LY
/// "Change this program", step 3): what a program sends, and its wire
/// form, which both ends build from this module.
pub mod handoff {
    use alloc::string::String;
    use alloc::vec::Vec;

    /// COPYDATASTRUCT's dwData for a hand-off ("gal1").
    pub const KIND: usize = 0x6761_6c31;
    /// The window the panel is: its class and title (FindWindowW).
    pub const CLASS: &str = "GalaUiWindow";
    pub const TITLE: &str = "Agent";
    /// The longest hand-off taken (bytes).
    pub const MAX: usize = 4096;

    /// The part of the window picked (Inspect): its automation id, the
    /// words for it, its rectangle in the window's client coordinates.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Element {
        pub id: String,
        pub label: String,
        pub rect: [i32; 4],
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Handoff {
        /// The program's name ("File Explorer").
        pub program: String,
        /// Its window's title.
        pub title: String,
        /// Its window (the sender's HWND).
        pub window: u64,
        pub text: String,
        pub element: Option<Element>,
    }

    /// Why a hand-off was not taken.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Refused {
        TooLong,
        NotUtf8,
        /// Not `gala-handoff 1` and its fields.
        Malformed,
    }

    const MAGIC: &str = "gala-handoff 1";

    /// The wire form: UTF-8, fields separated by NUL (no field holds
    /// one; a NUL typed into a field is dropped): the magic, program,
    /// title, window (hex), text, then the element's id, label and
    /// rectangle (x, y, w, h in decimal), empty when there is none.
    pub fn encode(h: &Handoff) -> Vec<u8> {
        let mut out = String::new();
        let mut field = |s: &str| {
            out.extend(s.chars().filter(|&c| c != '\0'));
            out.push('\0');
        };
        field(MAGIC);
        field(&h.program);
        field(&h.title);
        field(&alloc::format!("{:x}", h.window));
        field(&h.text);
        match &h.element {
            Some(e) => {
                field(&e.id);
                field(&e.label);
                let [x, y, w, hh] = e.rect;
                field(&alloc::format!("{x},{y},{w},{hh}"));
            }
            None => {
                field("");
                field("");
                field("");
            }
        }
        out.into_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Handoff, Refused> {
        if bytes.len() > MAX {
            return Err(Refused::TooLong);
        }
        let s = core::str::from_utf8(bytes).map_err(|_| Refused::NotUtf8)?;
        let s = s.strip_suffix('\0').ok_or(Refused::Malformed)?;
        let f: Vec<&str> = s.split('\0').collect();
        let [magic, program, title, window, text, id, label, rect] = f[..] else {
            return Err(Refused::Malformed);
        };
        if magic != MAGIC {
            return Err(Refused::Malformed);
        }
        let window = u64::from_str_radix(window, 16).map_err(|_| Refused::Malformed)?;
        let element = if id.is_empty() {
            None
        } else {
            let mut r = [0i32; 4];
            let mut parts = rect.split(',');
            for v in &mut r {
                *v = parts.next().and_then(|p| p.parse().ok()).ok_or(Refused::Malformed)?;
            }
            if parts.next().is_some() {
                return Err(Refused::Malformed);
            }
            Some(Element { id: id.into(), label: label.into(), rect: r })
        };
        Ok(Handoff {
            program: program.into(),
            title: title.into(),
            window,
            text: text.into(),
            element,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Hide,
    Show,
    Draft(String),
    Send,
    /// The active window changed (the program's, from its WinEvent).
    Context(Context),
    /// A program handed a request over (WM_COPYDATA).
    Handoff(handoff::Handoff),
    /// The model's reply to request `.0` (the program's worker thread).
    Answer(usize, Reply),
}

#[derive(Default)]
pub struct Panel {
    pub hidden: bool,
    /// None until a window other than the panel is active.
    pub context: Option<Context>,
    pub draft: String,
    pub requests: Vec<Request>,
}

/// The name a program goes by, from its file's name: gala's own
/// programs as Windows names them, any other by its file name without
/// `.exe`.
pub fn program_name(file: &str) -> String {
    let stem = file.rsplit(['\\', '/']).next().unwrap_or(file);
    let stem = match stem.len().checked_sub(4) {
        Some(n) if stem.is_char_boundary(n) && stem[n..].eq_ignore_ascii_case(".exe") => &stem[..n],
        _ => stem,
    };
    let known = [
        ("explorer", "File Explorer"),
        ("taskmgr", "Task Manager"),
        ("notepad", "Notepad"),
        ("cmd", "Command Prompt"),
        ("wsl", "Linux"),
    ];
    for (file, name) in known {
        if stem.eq_ignore_ascii_case(file) {
            return String::from(name);
        }
    }
    String::from(stem)
}

/// A request's reply card: "Reply{i}", named with what it says.
fn reply(i: usize, a: Option<&Answer>) -> Node<Msg> {
    let (t, color) = match a {
        Some(Answer::Said(t)) => (t.clone(), INK),
        Some(Answer::Failed(e)) => (alloc::format!("No answer: {e}"), FAILED),
        _ => (String::from("Asking the model..."), MUTED),
    };
    column()
        .padding(8)
        .radius(6)
        .bg(CHIP)
        .child(text(t).id(alloc::format!("Reply{i}")).size(13).color(color))
}

impl Panel {
    pub fn view(&self) -> Node<Msg> {
        if self.hidden {
            return column().id("AgentTab").bg(SURFACE).child(
                button("DockTab", "Agent")
                    .width(TAB.0)
                    .height(TAB.1)
                    .bg(ACCENT)
                    .color(0xffffff)
                    .size(13)
                    .bold()
                    .on_click(Msg::Show),
            );
        }
        let chip = match &self.context {
            Some(c) => row()
                .id("ContextChip")
                .gap(6)
                .padding((6, 12))
                .radius(14)
                .bg(CHIP)
                .align(Align::Center)
                .child(text(c.program.clone()).id("ContextProgram").bold().size(13))
                .child(text(c.title.clone()).id("ContextTitle").size(13).color(MUTED).ellipsis()),
            None => row()
                .id("ContextChip")
                .padding((6, 12))
                .radius(14)
                .bg(CHIP)
                .child(text("No window is active").id("ContextProgram").size(13).color(MUTED)),
        };
        let requests: Vec<Node<Msg>> = self
            .requests
            .iter()
            .enumerate()
            .map(|(i, r)| {
                column()
                    .id(alloc::format!("Request{i}"))
                    .gap(4)
                    .padding(10)
                    .radius(6)
                    .bg(CARD)
                    .border(LINE)
                    .child(text(r.program.clone()).size(12).bold().color(MUTED))
                    .child(text(r.text.clone()).id(alloc::format!("Request{i}Text")).size(14))
                    .child_if(
                        r.element.is_some(),
                        text(alloc::format!("On: {}", r.element.as_deref().unwrap_or("")))
                            .id(alloc::format!("Request{i}Element"))
                            .size(12)
                            .color(MUTED),
                    )
                    .child_if(r.answer.is_some(), reply(i, r.answer.as_ref()))
            })
            .collect();
        let empty = self.requests.is_empty();
        column()
            .id("AgentPanel")
            .bg(SURFACE)
            .border(LINE)
            .color(INK)
            .child(
                row()
                    .padding((8, 12))
                    .height(48)
                    .align(Align::Center)
                    .child(text("Agent").size(16).bold().grow(1))
                    .child(button("HideButton", "Hide").width(56).height(32).on_click(Msg::Hide)),
            )
            .child(row().padding((0, 12)).child(chip))
            .child(
                column()
                    .id("Requests")
                    .grow(1)
                    .gap(8)
                    .padding(12)
                    .child_if(
                        empty,
                        text("Ask for a change to the program above.").size(13).color(MUTED),
                    )
                    .children(requests),
            )
            .child(
                row()
                    .gap(8)
                    .padding(12)
                    .align(Align::Center)
                    .child(
                        input("Prompt", &self.draft)
                            .placeholder("Ask the agent")
                            .grow(1)
                            .height(32)
                            .on_input(Msg::Draft),
                    )
                    .child_if(
                        !self.draft.is_empty(),
                        button("SendButton", "Send")
                            .width(56)
                            .height(32)
                            .bg(ACCENT)
                            .color(0xffffff)
                            .on_click(Msg::Send),
                    ),
            )
    }

    /// The request just sent, to ask the model: its index and the body
    /// for the conversation so far (`chat::body`).
    pub fn asking(&self) -> Option<(usize, String)> {
        let i = self.requests.len().checked_sub(1)?;
        (self.requests[i].answer == Some(Answer::Asking))
            .then(|| (i, chat::body(self.context.as_ref(), &self.requests)))
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Hide => self.hidden = true,
            Msg::Show => self.hidden = false,
            Msg::Draft(t) => self.draft = t,
            Msg::Send => {
                let text = core::mem::take(&mut self.draft);
                if text.trim().is_empty() {
                    return;
                }
                let program = self.context.as_ref().map(|c| c.program.clone()).unwrap_or_default();
                self.requests.push(Request {
                    program,
                    text,
                    element: None,
                    answer: Some(Answer::Asking),
                });
            }
            Msg::Answer(i, r) => {
                // Only a request still being asked takes one.
                if let Some(a) = self.requests.get_mut(i).and_then(|q| q.answer.as_mut())
                    && *a == Answer::Asking
                {
                    *a = match r {
                        Ok(t) => Answer::Said(t),
                        Err(e) => Answer::Failed(e),
                    };
                }
            }
            Msg::Context(c) => self.context = Some(c),
            Msg::Handoff(h) => {
                self.hidden = false;
                self.context = Some(Context { program: h.program.clone(), title: h.title });
                self.requests.push(Request {
                    program: h.program,
                    text: h.text,
                    element: h.element.map(|e| e.label),
                    answer: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gala_ui::Placed;

    /// Half the size a character, one line: enough to place by.
    struct Fixed;
    impl gala_ui::Measure for Fixed {
        fn text(&mut self, t: &str, size: f32, _: bool, max: Option<f32>) -> (f32, f32) {
            let full = t.chars().count() as f32 * size * 0.5;
            (max.map_or(full, |m| full.min(m)), size + 4.0)
        }
    }

    fn lay(p: &Panel, w: i32, h: i32) -> Vec<Placed<Msg>> {
        gala_ui::place(&p.view(), w as f32, Some(h as f32), &mut Fixed).unwrap()
    }

    fn at<'a>(p: &'a [Placed<Msg>], id: &str) -> &'a Placed<Msg> {
        p.iter().find(|e| e.id.as_deref() == Some(id)).unwrap()
    }

    fn centre(e: &Placed<Msg>) -> (i32, i32) {
        (e.rect[0] + e.rect[2] / 2, e.rect[1] + e.rect[3] / 2)
    }

    #[test]
    fn names() {
        assert_eq!(program_name("C:\\Windows\\explorer.exe"), "File Explorer");
        assert_eq!(program_name("C:\\Windows\\System32\\Taskmgr.exe"), "Task Manager");
        assert_eq!(program_name("C:\\gala\\agent_test.exe"), "agent_test");
        assert_eq!(program_name("tool"), "tool");
    }

    #[test]
    fn hide_and_show() {
        let mut p = Panel::default();
        let placed = lay(&p, WIDTH, 800);
        // The Hide button, where the screen scenario clicks: 12 px from
        // the right edge, in the 48-row header.
        let hide = at(&placed, "HideButton");
        assert_eq!(hide.rect, [WIDTH - 12 - 56, 8, 56, 32]);
        let (x, y) = centre(hide);
        assert_eq!(gala_ui::click(&placed, x, y), Some(("HideButton", Msg::Hide)));
        p.update(Msg::Hide);
        let placed = lay(&p, TAB.0, TAB.1);
        let tab = at(&placed, "DockTab");
        assert_eq!(tab.rect, [0, 0, TAB.0, TAB.1]);
        assert!(placed.iter().all(|e| e.id.as_deref() != Some("AgentPanel")));
        assert_eq!(gala_ui::click(&placed, 20, 20), Some(("DockTab", Msg::Show)));
        p.update(Msg::Show);
        assert!(!p.hidden);
    }

    #[test]
    fn context_and_requests() {
        let mut p = Panel::default();
        assert_eq!(at(&lay(&p, WIDTH, 800), "ContextProgram").text, "No window is active");
        p.update(Msg::Context(Context { program: "File Explorer".into(), title: "tmp".into() }));
        let placed = lay(&p, WIDTH, 800);
        assert_eq!(at(&placed, "ContextProgram").text, "File Explorer");
        assert_eq!(at(&placed, "ContextTitle").text, "tmp");
        assert!(placed.iter().all(|e| e.id.as_deref() != Some("SendButton")));
        p.update(Msg::Draft("compress all the files".into()));
        let placed = lay(&p, WIDTH, 800);
        let (x, y) = centre(at(&placed, "SendButton"));
        assert_eq!(gala_ui::click(&placed, x, y), Some(("SendButton", Msg::Send)));
        p.update(Msg::Send);
        assert_eq!(
            p.requests,
            [Request {
                program: "File Explorer".into(),
                text: "compress all the files".into(),
                element: None,
                answer: Some(Answer::Asking),
            }]
        );
        assert!(p.draft.is_empty());
        // A blank request is not one.
        p.update(Msg::Draft("  ".into()));
        p.update(Msg::Send);
        assert_eq!(p.requests.len(), 1);
        let placed = lay(&p, WIDTH, 800);
        assert!(placed.iter().any(|e| e.id.as_deref() == Some("Request0")));
        // The prompt stays at the bottom.
        assert_eq!(at(&placed, "Prompt").rect[1] + 32 + 12, 800);
    }

    fn sample() -> handoff::Handoff {
        handoff::Handoff {
            program: "File Explorer".into(),
            title: "tmp".into(),
            window: 0x20010,
            text: "bigger, with\ntwo lines".into(),
            element: Some(handoff::Element {
                id: "AddressBar".into(),
                label: "the address bar".into(),
                rect: [64, -2, 600, 32],
            }),
        }
    }

    #[test]
    fn handoff_round_trips_and_refuses() {
        use handoff::{Refused, decode, encode};
        let h = sample();
        assert_eq!(decode(&encode(&h)), Ok(h.clone()));
        let none = handoff::Handoff { element: None, ..h.clone() };
        assert_eq!(decode(&encode(&none)), Ok(none));
        // A NUL typed into a field cannot split it.
        let nul = handoff::Handoff { text: "a\0b".into(), ..h.clone() };
        assert_eq!(decode(&encode(&nul)).map(|d| d.text), Ok("ab".into()));
        assert_eq!(decode(b"gala-handoff 2\0a\0b\x001\0t\0\0\0\0"), Err(Refused::Malformed));
        assert_eq!(decode(b"gala-handoff 1\0a\0b\0zz\0t\0\0\0\0"), Err(Refused::Malformed));
        assert_eq!(
            decode(b"gala-handoff 1\0a\0b\x001\0t\0id\0l\x001,2,3\0"),
            Err(Refused::Malformed)
        );
        assert_eq!(decode(b"gala-handoff 1\0a"), Err(Refused::Malformed));
        assert_eq!(decode(&[0xff, 0]), Err(Refused::NotUtf8));
        assert_eq!(decode(&[b'a'; handoff::MAX + 1]), Err(Refused::TooLong));
    }

    #[test]
    fn handoff_docks_and_lists_the_request() {
        let mut p = Panel { hidden: true, ..Panel::default() };
        p.update(Msg::Handoff(sample()));
        assert!(!p.hidden);
        assert_eq!(
            p.context,
            Some(Context { program: "File Explorer".into(), title: "tmp".into() })
        );
        let placed = lay(&p, WIDTH, 752);
        assert_eq!(at(&placed, "Request0Text").text, "bigger, with\ntwo lines");
        assert_eq!(at(&placed, "Request0Element").text, "On: the address bar");
        assert_eq!(at(&placed, "ContextProgram").text, "File Explorer");
    }

    #[test]
    fn a_sent_request_is_asked_and_answered() {
        let mut p = Panel::default();
        p.update(Msg::Context(Context { program: "Notepad".into(), title: "a.txt".into() }));
        assert_eq!(p.asking(), None);
        p.update(Msg::Draft("say pong".into()));
        p.update(Msg::Send);
        let (i, body) = p.asking().unwrap();
        assert_eq!(i, 0);
        assert!(body.ends_with("{\"role\":\"user\",\"content\":\"say pong\"}]}"), "{body}");
        assert_eq!(at(&lay(&p, WIDTH, 752), "Reply0").text, "Asking the model...");
        p.update(Msg::Answer(0, Ok("Pong".into())));
        assert_eq!(p.asking(), None);
        assert_eq!(at(&lay(&p, WIDTH, 752), "Reply0").text, "Pong");
        // A second answer to it, or one to no request, changes nothing.
        p.update(Msg::Answer(0, Ok("again".into())));
        p.update(Msg::Answer(7, Ok("none".into())));
        assert_eq!(p.requests[0].answer, Some(Answer::Said("Pong".into())));
        // The next is asked with the conversation so far.
        p.update(Msg::Draft("and?".into()));
        p.update(Msg::Send);
        let (i, body) = p.asking().unwrap();
        assert_eq!(i, 1);
        assert!(body.contains("\"assistant\",\"content\":\"Pong\""), "{body}");
        p.update(Msg::Answer(1, Err("agent-relay: not recorded".into())));
        let placed = lay(&p, WIDTH, 752);
        assert_eq!(at(&placed, "Reply1").text, "No answer: agent-relay: not recorded");
        // A hand-off is listed, not asked.
        p.update(Msg::Handoff(sample()));
        assert_eq!(p.asking(), None);
        assert!(lay(&p, WIDTH, 752).iter().all(|e| e.id.as_deref() != Some("Reply2")));
    }
}
