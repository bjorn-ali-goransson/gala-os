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
}
