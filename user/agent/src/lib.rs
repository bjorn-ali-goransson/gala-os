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
//! The program (main.rs) does what a message means outside the view:
//! the app bar's strip, the window's place, the remembered state.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    Hide,
    Show,
    Draft(String),
    Send,
    /// The active window changed (the program's, from its WinEvent).
    Context(Context),
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
                    .child(text(r.text.clone()).size(14))
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
                self.requests.push(Request { program, text });
            }
            Msg::Context(c) => self.context = Some(c),
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
            [Request { program: "File Explorer".into(), text: "compress all the files".into() }]
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
}
