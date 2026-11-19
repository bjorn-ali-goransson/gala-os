//! "Change this program..." (ROADMAP "Change this program: a coding agent
//! from the window menu"; ARCHITECTURE "The file explorer"): the flow's
//! state, with no coding agent behind it yet. The window menu's item
//! opens a request card; Inspect picks the part of the window the
//! request is about (an element by its automation id, outlined as the
//! pointer moves); "Make the change" walks the steps an agent would take
//! on a clock ([`STEPS`], [`STEP_MS`] each) and ends on a note that
//! nothing was changed. The window's drawing and hit testing are the
//! program's (`main.rs`); what is here runs under `cargo test`.

use alloc::string::String;
use core::fmt::Write;
use gala_ui::{Align, Justify, Node, button, column, row, text};

use crate::STATUS_H;

/// The steps the progress card lists, in order.
pub const STEPS: [&str; 4] = [
    "Reading File Explorer's source",
    "Editing the files",
    "Building File Explorer",
    "Restarting File Explorer",
];

/// How long each step shows as under way (milliseconds).
pub const STEP_MS: u32 = 900;

/// The longest request kept (characters).
pub const MAX_REQUEST: usize = 400;

/// A part of the window: its automation id (what an agent would map to
/// the source), the words the card shows for it, its rectangle in client
/// coordinates (x, y, w, h).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: String,
    pub label: String,
    pub rect: [i32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The request card: the text being typed, the part picked.
    Request,
    /// Inspect: the card is away, the element under the pointer outlined;
    /// a click picks it.
    Inspecting,
    /// The steps under way since `started` (GetTickCount).
    Working { started: u32 },
    /// The end: nothing was changed (no agent).
    Done,
}

/// What [`Change::key`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyResult {
    Nothing,
    /// The text changed (or the card is unchanged but needs drawing).
    Edited,
    /// Enter on a request: the steps start.
    Submitted,
    /// Esc while inspecting: back to the card.
    InspectCancelled,
    /// Esc (or Enter on the last card): the flow is over.
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub stage: Stage,
    pub request: String,
    /// The part picked (None: the whole window).
    pub target: Option<Element>,
    /// While inspecting: the element under the pointer.
    pub hover: Option<Element>,
}

impl Default for Change {
    fn default() -> Self {
        Self::new()
    }
}

impl Change {
    pub fn new() -> Change {
        Change { stage: Stage::Request, request: String::new(), target: None, hover: None }
    }

    /// The request can be sent: some words typed.
    pub fn can_submit(&self) -> bool {
        self.stage == Stage::Request && !self.request.trim().is_empty()
    }

    /// The steps start at `now` (if the request can be sent).
    pub fn submit(&mut self, now: u32) -> bool {
        if !self.can_submit() {
            return false;
        }
        self.stage = Stage::Working { started: now };
        true
    }

    /// How many steps are done at `now` (all of them once Done).
    pub fn steps_done(&self, now: u32) -> usize {
        match self.stage {
            Stage::Working { started } => {
                ((now.wrapping_sub(started) / STEP_MS) as usize).min(STEPS.len())
            }
            Stage::Done => STEPS.len(),
            _ => 0,
        }
    }

    /// The clock moved to `now`: Done once every step is. The steps that
    /// started since `before` (for the log): every one, also when a late
    /// tick passes several.
    pub fn tick(&mut self, before: u32, now: u32) -> core::ops::Range<usize> {
        let Stage::Working { .. } = self.stage else { return 0..0 };
        let (was, is) = (self.steps_done(before), self.steps_done(now));
        if is == STEPS.len() {
            self.stage = Stage::Done;
        }
        (was + 1).min(STEPS.len())..(is + 1).min(STEPS.len())
    }

    /// Inspect: the card goes away until a part is picked or Esc.
    pub fn inspect(&mut self) {
        if self.stage == Stage::Request {
            self.stage = Stage::Inspecting;
            self.hover = None;
        }
    }

    /// While inspecting, the pointer is over `e`: whether that changed
    /// what is outlined.
    pub fn hover(&mut self, e: Option<Element>) -> bool {
        if self.stage != Stage::Inspecting || self.hover == e {
            return false;
        }
        self.hover = e;
        true
    }

    /// While inspecting, a click on `e`: it is the request's part.
    pub fn pick(&mut self, e: Option<Element>) {
        if self.stage != Stage::Inspecting {
            return;
        }
        if e.is_some() {
            self.target = e;
        }
        self.hover = None;
        self.stage = Stage::Request;
    }

    /// A typed character (WM_CHAR).
    pub fn key(&mut self, c: u16) -> KeyResult {
        match (self.stage, c) {
            (Stage::Inspecting, 0x1b) => {
                self.hover = None;
                self.stage = Stage::Request;
                KeyResult::InspectCancelled
            }
            (Stage::Done, 0x1b | 0x0d) | (Stage::Request, 0x1b) => KeyResult::Closed,
            (Stage::Request, 0x0d) if self.can_submit() => KeyResult::Submitted,
            (Stage::Request, 0x08) => {
                self.request.pop();
                KeyResult::Edited
            }
            (Stage::Request, 0x20..) if self.request.chars().count() < MAX_REQUEST => {
                match char::from_u32(u32::from(c)) {
                    Some(ch) => {
                        self.request.push(ch);
                        KeyResult::Edited
                    }
                    None => KeyResult::Nothing,
                }
            }
            _ => KeyResult::Nothing,
        }
    }

    /// The words for the part: the element's, or the whole window.
    pub fn target_label(&self) -> &str {
        self.target.as_ref().map_or("The whole window", |e| e.label.as_str())
    }

    /// The part's automation id for the log ("" for the whole window).
    pub fn target_id(&self) -> &str {
        self.target.as_ref().map_or("", |e| e.id.as_str())
    }
}

// The cards and Inspect's outline as a gala_ui view (ARCHITECTURE "UI
// components"): Explorer lays it out over its window and paints it with
// `gala_ui::win::Gdi`; a click goes to `gala_ui::click`, whose message
// main.rs acts on. Colours 0xRRGGBB, Explorer's own.

/// What a click on the cards asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    Inspect,
    Cancel,
    Make,
    Close,
}

pub const CARD_W: i32 = 520;
/// The request box's least height; a longer request grows it (to at most
/// [`MAX_REQUEST`] characters).
pub const BOX_H: i32 = 96;
const PAD: i32 = 20;
const BUTTON_H: i32 = 32;
const ACCENT: u32 = 0x005fb8;
const ON_ACCENT: u32 = 0xffffff;
const CARD_BG: u32 = 0xf9f9f9;
const CONTENT: u32 = 0xffffff;
const TEXT: u32 = 0x1b1b1b;
const TEXT_DIM: u32 = 0x5d5d5d;
const LINE: u32 = 0xe5e5e5;
const BORDER: u32 = 0xa0a0a0;
const CONTROL_BORDER: u32 = 0xd0d0d0;
const TAG_BG: u32 = 0x1b1b1b;
const FONT: i32 = 13;
const SMALL: i32 = 12;

fn primary(id: &str, label: &str, width: i32, msg: Option<Msg>) -> Node<Msg> {
    let b = button(id, label).width(width).height(BUTTON_H);
    match msg {
        Some(m) => b.bg(ACCENT).color(ON_ACCENT).on_click(m),
        None => b.bg(LINE).color(TEXT_DIM),
    }
}

fn secondary(id: &str, label: &str, width: i32, msg: Msg) -> Node<Msg> {
    button(id, label)
        .width(width)
        .height(BUTTON_H)
        .bg(CONTENT)
        .border(CONTROL_BORDER)
        .color(TEXT)
        .on_click(msg)
}

/// Inspect, with a crosshair before its label: the button, then the
/// crosshair over it (a circle and four ticks, no ids: a click on it is
/// the button's). The label is centred between the icon and as much
/// space on the right less 6 px, so icon and label sit centred together.
fn inspect_button() -> Node<Msg> {
    const W: i32 = 96;
    const ICON: i32 = 15;
    const AT: i32 = 15;
    let tick = |x: i32, y: i32, w: i32, h: i32| column().pin(x, y).width(w).height(h).bg(TEXT);
    let (mid, arm) = (ICON / 2, ICON / 3);
    row()
        .width(W)
        .height(BUTTON_H)
        .child(secondary("Inspect", "Inspect", W, Msg::Inspect).padding((0, AT - 6, 0, AT + ICON)))
        .child(
            column()
                .pin(AT, (BUTTON_H - ICON) / 2)
                .width(ICON)
                .height(ICON)
                .child(
                    column()
                        .pin(2, 2)
                        .width(ICON - 4)
                        .height(ICON - 4)
                        .border(TEXT)
                        .radius((ICON - 4) / 2),
                )
                .child(tick(0, mid, arm, 1))
                .child(tick(ICON - arm, mid, arm, 1))
                .child(tick(mid, 0, 1, arm))
                .child(tick(mid, ICON - arm, 1, arm)),
        )
}

fn small(t: impl Into<String>) -> Node<Msg> {
    text(t).size(SMALL).color(TEXT_DIM)
}

/// The flow over a window of `size` (client pixels) at `now`
/// (GetTickCount): the card centred, or while inspecting the band over
/// the status bar and the outline of the part under the pointer.
pub fn view(c: &Change, size: (i32, i32), now: u32) -> Node<Msg> {
    let (w, h) = size;
    let root = column().width(w).height(h).size(FONT).color(TEXT);
    if c.stage == Stage::Inspecting {
        return inspecting(c, root, w, h);
    }
    let body = match c.stage {
        Stage::Request | Stage::Inspecting => request(c),
        Stage::Working { started } => working(c, started, now),
        Stage::Done => done(),
    };
    let card = column()
        .id("ChangeCard")
        .width(CARD_W)
        .bg(CARD_BG)
        .border(BORDER)
        .child(column().height(3).bg(ACCENT))
        .child(body.padding((15, PAD, 16, PAD)).gap(8));
    root.align(Align::Center).justify(Justify::Center).child(card)
}

fn request(c: &Change) -> Node<Msg> {
    let mut typed = c.request.clone();
    typed.push('|');
    column()
        .child(text("Change File Explorer").id("Title").bold())
        .child(small("Describe what you would like File Explorer to do differently."))
        .child(
            // The request box: what is typed and the caret, the accent
            // bar along its bottom.
            column()
                .id("RequestBox")
                .margin((4, 0, 4, 0))
                .bg(CONTENT)
                .border(CONTROL_BORDER)
                .child(text(typed).id("Request").min_height(BOX_H - 2).padding((8, 10)))
                .child(column().height(2).bg(ACCENT).margin((0, 1, 1, 1))),
        )
        .child(small("Part"))
        .child(
            row()
                .gap(12)
                .align(Align::Center)
                .child(text(c.target_label()).id("Part").grow(1))
                .child(inspect_button()),
        )
        .child(small("Preview: no coding agent is connected yet, so nothing will be changed."))
        .child(
            row()
                .margin((8, 0, 0, 0))
                .gap(8)
                .justify(Justify::End)
                .child(secondary("Cancel", "Cancel", 90, Msg::Cancel))
                .child(primary(
                    "MakeChange",
                    "Make the change",
                    150,
                    c.can_submit().then_some(Msg::Make),
                )),
        )
}

fn working(c: &Change, started: u32, now: u32) -> Node<Msg> {
    let done = c.steps_done(now);
    let mut quoted = String::from("\"");
    quoted.push_str(&c.request);
    quoted.push('"');
    let mut steps = column().id("Steps").margin((8, 0, 8, 0)).gap(12);
    for (k, step) in STEPS.iter().enumerate() {
        let id = |what: &str| {
            let mut s = String::from("Step");
            s.push(char::from(b'0' + k as u8));
            s.push_str(what);
            s
        };
        let marker = column().id(id("Marker")).width(12).height(12);
        let marker = if k < done {
            marker.bg(ACCENT)
        } else if k == done {
            marker.border(ACCENT).padding(3).child(column().height(6).bg(ACCENT))
        } else {
            marker.border(BORDER)
        };
        let mut r =
            row().gap(12).align(Align::Center).child(marker).child(
                text(*step).id(id("")).grow(1).color(if k <= done { TEXT } else { TEXT_DIM }),
            );
        if k == done {
            let ms = now.wrapping_sub(started) % STEP_MS;
            let mut secs = String::new();
            let _ = write!(secs, "{}.{} s", ms / 1000, ms / 100 % 10);
            r = r.child(small(secs).id(id("Time")));
        }
        steps = steps.child(r);
    }
    let inner = CARD_W - 2 * PAD;
    column()
        .child(text("Changing File Explorer").id("Title").bold())
        .child(small(quoted).id("Quoted"))
        .child(steps)
        .child(
            column().id("Progress").height(4).bg(LINE).align(Align::Start).child(
                column()
                    .id("ProgressDone")
                    .width(inner * done as i32 / STEPS.len() as i32)
                    .height(4)
                    .bg(ACCENT),
            ),
        )
}

fn done() -> Node<Msg> {
    column()
        .child(text("Nothing was changed").id("Title").bold())
        .child(text("No coding agent is connected yet, so File Explorer was not changed."))
        .child(text(
            "When one is, it will edit File Explorer's source, build it and restart it here, \
             keeping your folder and selection.",
        ))
        .child(row().margin((8, 0, 0, 0)).justify(Justify::End).child(primary(
            "Close",
            "Close",
            90,
            Some(Msg::Close),
        )))
}

/// Inspect: a band over the status bar says what to do; the part under
/// the pointer gets a two-pixel outline and its id in a tag above it
/// (under it at the window's top).
fn inspecting(c: &Change, root: Node<Msg>, w: i32, h: i32) -> Node<Msg> {
    let band = row()
        .id("InspectBand")
        .pin(0, h - STATUS_H)
        .width(w)
        .height(STATUS_H)
        .bg(ACCENT)
        .align(Align::Center)
        .padding((0, 12))
        .child(
            text("Inspect: click the part of File Explorer you want to change. Esc goes back.")
                .size(SMALL)
                .color(ON_ACCENT),
        );
    let Some(e) = &c.hover else { return root.child(band) };
    let [x, y, ew, eh] = e.rect;
    let outline =
        column().id("Outline").pin(x, y).width(ew).height(eh).border(ACCENT).child(
            column().id("OutlineInner").pin(1, 1).width(ew - 2).height(eh - 2).border(ACCENT),
        );
    let ty = if y >= 22 { y - 22 } else { y + eh + 2 };
    // A row as wide as the room to the right, the tag as wide as its id.
    let tag = row().pin(x, ty).width((w - x).max(1)).align(Align::Start).child(
        text(e.id.as_str())
            .id("Tag")
            .size(SMALL)
            .color(ON_ACCENT)
            .bg(TAG_BG)
            .padding((2, 8))
            .height(20),
    );
    root.child(band).child(outline).child(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(id: &str) -> Element {
        Element { id: String::from(id), label: String::from(id), rect: [0, 0, 10, 10] }
    }

    fn typed(c: &mut Change, text: &str) {
        for ch in text.encode_utf16() {
            c.key(ch);
        }
    }

    #[test]
    fn a_request_needs_words_before_it_is_sent() {
        let mut c = Change::new();
        assert_eq!(c.key(0x0d), KeyResult::Nothing);
        typed(&mut c, "   ");
        assert!(!c.submit(0));
        assert_eq!(c.stage, Stage::Request);
        typed(&mut c, "a column");
        assert_eq!(c.request, "   a column");
        assert_eq!(c.key(0x08), KeyResult::Edited);
        assert_eq!(c.request, "   a colum");
        assert_eq!(c.key(0x0d), KeyResult::Submitted);
        assert!(c.submit(1000));
        assert_eq!(c.stage, Stage::Working { started: 1000 });
        // Typing no longer edits it.
        assert_eq!(c.key(u16::from(b'x')), KeyResult::Nothing);
    }

    #[test]
    fn the_request_is_kept_to_its_length() {
        let mut c = Change::new();
        for _ in 0..MAX_REQUEST + 10 {
            c.key(u16::from(b'a'));
        }
        assert_eq!(c.request.len(), MAX_REQUEST);
    }

    #[test]
    fn the_steps_run_on_the_clock_and_end_done() {
        let mut c = Change::new();
        typed(&mut c, "sort by size");
        // Near the counter's wrap: the elapsed time still counts.
        let start = u32::MAX - 100;
        assert!(c.submit(start));
        assert_eq!(c.steps_done(start), 0);
        assert!(c.tick(start, start.wrapping_add(STEP_MS - 1)).is_empty());
        assert_eq!(c.tick(start, start.wrapping_add(STEP_MS)), 1..2);
        assert_eq!(c.steps_done(start.wrapping_add(2 * STEP_MS + 5)), 2);
        // A late tick: every step it passed is reported.
        assert_eq!(c.tick(start.wrapping_add(STEP_MS), start.wrapping_add(3 * STEP_MS)), 2..4);
        let end = start.wrapping_add(STEPS.len() as u32 * STEP_MS);
        assert!(c.tick(start.wrapping_add(3 * STEP_MS), end).is_empty());
        assert_eq!(c.stage, Stage::Done);
        assert_eq!(c.steps_done(0), STEPS.len());
        // The last card: Enter or Esc closes it.
        assert_eq!(c.key(0x0d), KeyResult::Closed);
    }

    #[test]
    fn inspect_outlines_and_picks_a_part_or_esc_keeps_the_last() {
        let mut c = Change::new();
        assert_eq!(c.target_label(), "The whole window");
        // Not while there is no card for it.
        assert!(!c.hover(Some(element("x"))));
        c.inspect();
        assert_eq!(c.stage, Stage::Inspecting);
        assert!(c.hover(Some(element("ColumnHeader.Name"))));
        assert!(!c.hover(Some(element("ColumnHeader.Name"))));
        c.pick(Some(element("ColumnHeader.Name")));
        assert_eq!(
            (c.stage, c.target_id(), c.hover.is_none()),
            (Stage::Request, "ColumnHeader.Name", true)
        );
        // Again, then Esc: the part picked before stays.
        c.inspect();
        c.hover(Some(element("StatusBar")));
        assert_eq!(c.key(0x1b), KeyResult::InspectCancelled);
        assert_eq!(c.target_id(), "ColumnHeader.Name");
        // A click on nothing picks nothing.
        c.inspect();
        c.pick(None);
        assert_eq!(c.target_id(), "ColumnHeader.Name");
        // Esc on the card closes the flow.
        assert_eq!(c.key(0x1b), KeyResult::Closed);
    }

    /// 7 pixels a character, lines of 18, wrapped at word boundaries.
    struct Fixed;
    impl gala_ui::Measure for Fixed {
        fn text(&mut self, t: &str, _: f32, _: bool, max: Option<f32>) -> (f32, f32) {
            let full = t.chars().count() as f32 * 7.0;
            match max {
                Some(m) if full > m && m > 0.0 => (m, 18.0 * (full / m).ceil()),
                _ => (full, 18.0),
            }
        }
    }

    const W: i32 = 1000;
    const H: i32 = 700;

    fn lay(c: &Change, now: u32) -> alloc::vec::Vec<gala_ui::Placed<Msg>> {
        gala_ui::place(&view(c, (W, H), now), W as f32, Some(H as f32), &mut Fixed).unwrap()
    }

    fn at<'a>(p: &'a [gala_ui::Placed<Msg>], id: &str) -> &'a gala_ui::Placed<Msg> {
        p.iter().find(|x| x.id.as_deref() == Some(id)).unwrap_or_else(|| panic!("no {id}"))
    }

    fn has(p: &[gala_ui::Placed<Msg>], id: &str) -> bool {
        p.iter().any(|x| x.id.as_deref() == Some(id))
    }

    fn centre(p: &gala_ui::Placed<Msg>) -> (i32, i32) {
        (p.rect[0] + p.rect[2] / 2, p.rect[1] + p.rect[3] / 2)
    }

    #[test]
    fn the_request_card_is_centred_and_its_buttons_send_their_messages() {
        let mut c = Change::new();
        let p = lay(&c, 0);
        let card = at(&p, "ChangeCard").rect;
        assert_eq!(card[2], CARD_W);
        // Centred (to the half pixel an odd height leaves).
        assert_eq!(card[0], (W - CARD_W) / 2);
        assert!((2 * card[1] + card[3] - H).abs() <= 1, "{card:?}");
        assert_eq!(at(&p, "Request").text, "|");
        assert!(at(&p, "RequestBox").rect[3] >= BOX_H);
        assert_eq!(at(&p, "Part").text, "The whole window");
        let (x, y) = centre(at(&p, "Inspect"));
        assert_eq!(gala_ui::click(&p, x, y), Some(("Inspect", Msg::Inspect)));
        // The crosshair: a ring before the label, and a click on it is
        // the button's.
        let b = at(&p, "Inspect").rect;
        let ring = p.iter().find(|e| e.radius > 0 && e.border == Some(TEXT)).expect("ring");
        assert_eq!(ring.rect, [b[0] + 17, b[1] + 10, 11, 11]);
        assert_eq!(gala_ui::click(&p, b[0] + 22, b[1] + 16), Some(("Inspect", Msg::Inspect)));
        let (x, y) = centre(at(&p, "Cancel"));
        assert_eq!(gala_ui::click(&p, x, y), Some(("Cancel", Msg::Cancel)));
        // Nothing typed: Make the change is greyed and sends nothing.
        let make = at(&p, "MakeChange");
        assert_eq!((make.background, make.on_click), (Some(LINE), None));
        typed(&mut c, "Bigger icons");
        let p = lay(&c, 0);
        let make = at(&p, "MakeChange");
        assert_eq!((make.background, make.on_click), (Some(ACCENT), Some(Msg::Make)));
        assert_eq!(at(&p, "Request").text, "Bigger icons|");
        // A long request grows the box, and the card with it.
        typed(&mut c, &"word ".repeat(60));
        let q = lay(&c, 0);
        assert!(at(&q, "RequestBox").rect[3] > at(&p, "RequestBox").rect[3]);
        assert!(at(&q, "ChangeCard").rect[3] > card[3]);
    }

    #[test]
    fn inspect_draws_the_band_the_outline_and_the_tag_and_no_card() {
        let mut c = Change::new();
        c.inspect();
        let p = lay(&c, 0);
        assert!(!has(&p, "ChangeCard"));
        assert_eq!(at(&p, "InspectBand").rect, [0, H - STATUS_H, W, STATUS_H]);
        assert!(!has(&p, "Outline"));
        let e = Element { id: "StatusBar".into(), label: "x".into(), rect: [100, 200, 300, 40] };
        c.hover(Some(e));
        let p = lay(&c, 0);
        assert_eq!(at(&p, "Outline").rect, [100, 200, 300, 40]);
        assert_eq!(at(&p, "Outline").border, Some(ACCENT));
        // Two pixels: a second frame inside the first.
        assert_eq!(at(&p, "OutlineInner").rect, [101, 201, 298, 38]);
        let tag = at(&p, "Tag");
        assert_eq!((tag.text.as_str(), tag.background), ("StatusBar", Some(TAG_BG)));
        assert_eq!((tag.rect[0], tag.rect[1] + tag.rect[3]), (100, 198));
        // Its width is its text's (and padding), not the window's.
        assert_eq!(tag.rect[2], 9 * 7 + 16);
        // At the top, the tag goes under the element.
        let e = Element { id: "Top".into(), label: "x".into(), rect: [0, 4, 50, 30] };
        c.hover(Some(e));
        assert_eq!(at(&lay(&c, 0), "Tag").rect[1], 4 + 30 + 2);
        // The band and the outline send nothing: the click is Explorer's.
        assert_eq!(gala_ui::click(&p, 5, H - 5), None);
    }

    #[test]
    fn the_steps_show_what_is_done_what_is_under_way_and_the_bar() {
        let mut c = Change::new();
        typed(&mut c, "x");
        c.submit(1000);
        // 2.3 s in: two steps done, the third 0.5 s under way.
        let p = lay(&c, 1000 + 2 * STEP_MS + 500);
        assert_eq!(at(&p, "Step0Marker").background, Some(ACCENT));
        assert_eq!(at(&p, "Step2Marker").border, Some(ACCENT));
        assert_eq!(at(&p, "Step3Marker").border, Some(BORDER));
        assert_eq!(at(&p, "Step2Time").text, "0.5 s");
        assert!(!has(&p, "Step1Time"));
        assert_eq!(at(&p, "Step3").color, TEXT_DIM);
        assert_eq!(at(&p, "Progress").rect[2], CARD_W - 40);
        assert_eq!(at(&p, "ProgressDone").rect[2], (CARD_W - 40) * 2 / 4);
        assert!(!has(&p, "MakeChange"));
    }

    #[test]
    fn the_last_card_closes() {
        let mut c = Change::new();
        typed(&mut c, "x");
        c.submit(0);
        c.tick(0, 10 * STEP_MS);
        assert_eq!(c.stage, Stage::Done);
        let p = lay(&c, 10 * STEP_MS);
        let (x, y) = centre(at(&p, "Close"));
        assert_eq!(gala_ui::click(&p, x, y), Some(("Close", Msg::Close)));
        assert!(at(&p, "ChangeCard").text.is_empty());
    }
}
