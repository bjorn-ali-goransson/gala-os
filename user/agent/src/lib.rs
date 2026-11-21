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
//!   the request with the part of the window it is about. A process
//!   handed over (Task Manager's "Ask the agent about this process",
//!   [`handoff::About`]) docks the panel too, is named by the context
//!   chip and listed ("About: <name> (pid N)"), and its text goes to the
//!   model with every request asked while it is the panel's subject.
//!
//! - **Chat** ([`chat`]): a request typed and sent is asked of the model
//!   (the program POSTs `chat::body` on a worker thread); its reply card
//!   ("Reply{i}") says it is being asked, then the answer, or why there
//!   is none.
//!
//! - **Tools**: the model is offered one tool, `run` (a command line run
//!   by cmd.exe in the working folder: the folder the active program
//!   shows, which File Explorer reports by hand-off,
//!   [`handoff::Folder`]). Each call it makes is a step in the request's
//!   card ("Request{i}Step{n}") with the command, the folder and the
//!   model's explanation, and nothing runs until the user clicks its Run
//!   button (Cancel tells the model they declined). Once every call of a
//!   round has a result the model is asked again with them, for at most
//!   `ROUNDS` calls of the model per request.
//!
//! The program (main.rs) does what a message means outside the view:
//! the app bar's strip, the window's place, the remembered state, and
//! what is due (`Panel::take_due`): the call to the model, the command
//! to run.

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
const OK: u32 = 0x0f7b0f;
/// The prompt's frame: darker than a card's, so the box reads as one
/// to type in.
const INPUT_LINE: u32 = 0x8a8a8a;

/// The most calls of the model one request makes: its first, and one
/// after each round of tool calls; a request whose model still calls the
/// tool after these fails ("stopped after N rounds").
pub const ROUNDS: usize = 8;
/// The most windows whose folders the panel keeps (the oldest goes).
pub const FOLDERS: usize = 16;
/// The most of a step's output its card shows (characters); the model
/// is given all that was kept (`chat::MAX_OUTPUT`).
const SHOWN: usize = 600;

/// What the panel is about: the active window's program and title.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Context {
    /// The program's name ("File Explorer", else its file's name).
    pub program: String,
    /// The window's title.
    pub title: String,
    /// The window (its HWND; 0 when not known), which a folder reported
    /// for it is keyed by.
    pub window: u64,
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
    /// Where `run` runs for it: the working folder when it was sent.
    pub work: Work,
    /// The model's calls of the tool, a round per reply that made them.
    pub rounds: Vec<Round>,
    /// How many times the model was called for it.
    pub asked: usize,
}

impl Request {
    fn new(program: String, text: String, element: Option<String>, work: Work) -> Request {
        Request { program, text, element, answer: None, work, rounds: Vec::new(), asked: 0 }
    }
}

/// A request's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The model is being called, or its calls of the tool are being
    /// carried out (`Request::rounds`).
    Asking,
    Said(String),
    /// Why there is no answer (the relay's or the network's error, or
    /// the loop's limit).
    Failed(String),
}

/// What asking the model gave: what it said, or why there is nothing.
pub type Reply = Result<chat::Said, String>;

/// Where a command runs: the folder the request's program showed when
/// it was sent, or why there is none to run in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// A Windows folder (`C:\...`): `cmd.exe /c` runs there.
    Windows {
        program: String,
        folder: String,
    },
    /// A folder of a Linux distribution (`\\wsl.localhost\<distro>\...`):
    /// running there is not supported yet.
    Linux {
        program: String,
        distro: String,
        path: String,
    },
    Unknown(String),
}

/// What this machine has to run with, found by the program at start.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Tools {
    /// cmd.exe's path (in System32), None when it is not installed.
    pub cmd: Option<String>,
    /// Whether busybox.exe is on the PATH (its tar and gzip compress).
    pub busybox: bool,
}

/// A reply's calls of the tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Round {
    /// What the model said beside them, if anything.
    pub text: Option<String>,
    pub calls: Vec<Call>,
}

/// A call of the tool: as the model made it (given back to it as it
/// was), what it asks to run, and where it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub command: String,
    pub explanation: String,
    pub step: Step,
    /// Whether the user opened its details (`Msg::Toggle`): a step that
    /// is waiting or running shows them anyway, one that ended shows a
    /// line until opened.
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Shown, waiting for the user's Run or Cancel.
    Waiting,
    Running,
    Ran(Ran),
    /// The user clicked Cancel.
    Declined,
    /// Not runnable, so not shown with Run (a tool that is not `run`,
    /// arguments that are not its, no working folder): why.
    Refused(String),
}

/// What running a command gave: its exit code, the output kept (at most
/// `chat::MAX_OUTPUT` bytes of standard output and error, as text) and
/// how many bytes there were in all; or, as `code` None, why it did not
/// start or could not be waited for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    pub code: Option<u32>,
    pub output: String,
    pub bytes: usize,
}

/// What the program is to do next for the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Due {
    /// Call the model for request `index` with `body` (`round` of
    /// `ROUNDS`, from 1).
    Ask { index: usize, round: usize, body: String },
    /// Run `command` (`cmd.exe /c`) in `folder` for request `index`'s
    /// step `step`.
    Run { index: usize, step: usize, cmd: String, command: String, folder: String },
}

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

    /// A process handed over to be asked about (Task Manager's "Ask the
    /// agent about this process"): who sent it, the process, and the
    /// text Task Manager gives for its row (`taskmgr::row_text`).
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct About {
        /// The sender's name ("Task Manager"), its window's title and
        /// its window.
        pub program: String,
        pub title: String,
        pub window: u64,
        /// The process: its name (its image), pid and personality.
        pub name: String,
        pub pid: u64,
        pub linux: bool,
        /// Its command line, empty where it is not known.
        pub command_line: String,
        /// The row as text: its values and their recent history.
        pub text: String,
    }

    impl About {
        /// Its personality as the wire form and the panel say it.
        pub fn personality(&self) -> &'static str {
            if self.linux { "Linux" } else { "Windows" }
        }
    }

    /// The folder a window shows (File Explorer's current folder, sent
    /// on each navigation and when the window is activated): its program,
    /// title and window, and the folder's file-system path, empty when
    /// it shows none (Home, This PC).
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Folder {
        pub program: String,
        pub title: String,
        pub window: u64,
        pub path: String,
    }

    /// What a hand-off is: a request on a program, a process to ask
    /// about, or the folder a window shows.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Received {
        Request(Handoff),
        About(About),
        Folder(Folder),
    }

    /// Why a hand-off was not taken.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Refused {
        TooLong,
        NotUtf8,
        /// Not one of the forms below (`gala-handoff 1`, `gala-process
        /// 1`, `gala-folder 1`) and its fields.
        Malformed,
    }

    const MAGIC: &str = "gala-handoff 1";
    const MAGIC_ABOUT: &str = "gala-process 1";
    const MAGIC_FOLDER: &str = "gala-folder 1";

    /// Fields `f` as the wire form: each without NULs, each ended by one.
    fn fields(f: &[&str]) -> Vec<u8> {
        let mut out = String::new();
        for s in f {
            out.extend(s.chars().filter(|&c| c != '\0'));
            out.push('\0');
        }
        out.into_bytes()
    }

    /// The fields of a hand-off of at most `MAX` bytes of UTF-8, each
    /// ended by a NUL.
    fn split(bytes: &[u8]) -> Result<Vec<&str>, Refused> {
        if bytes.len() > MAX {
            return Err(Refused::TooLong);
        }
        let s = core::str::from_utf8(bytes).map_err(|_| Refused::NotUtf8)?;
        let s = s.strip_suffix('\0').ok_or(Refused::Malformed)?;
        Ok(s.split('\0').collect())
    }

    /// A process to ask about, in its wire form: the magic `gala-process
    /// 1`, the sender's program, title and window (hex), the process's
    /// name, pid (decimal), personality (`Windows` or `Linux`), command
    /// line and text, each ended by a NUL as a request's fields are. Its
    /// own magic, so a request's form (`encode`) is unchanged and a
    /// panel that knows only requests refuses it as Malformed.
    pub fn encode_about(a: &About) -> Vec<u8> {
        fields(&[
            MAGIC_ABOUT,
            &a.program,
            &a.title,
            &alloc::format!("{:x}", a.window),
            &a.name,
            &alloc::format!("{}", a.pid),
            a.personality(),
            &a.command_line,
            &a.text,
        ])
    }

    /// The folder a window shows, in its wire form: the magic
    /// `gala-folder 1`, the program, title, window (hex) and path, each
    /// ended by a NUL.
    pub fn encode_folder(f: &Folder) -> Vec<u8> {
        fields(&[MAGIC_FOLDER, &f.program, &f.title, &alloc::format!("{:x}", f.window), &f.path])
    }

    /// Any form, by its magic.
    pub fn receive(bytes: &[u8]) -> Result<Received, Refused> {
        let f = split(bytes)?;
        match f.first() {
            Some(&MAGIC) => decode(bytes).map(Received::Request),
            Some(&MAGIC_ABOUT) => {
                let [_, program, title, window, name, pid, personality, command_line, text] = f[..]
                else {
                    return Err(Refused::Malformed);
                };
                let linux = match personality {
                    "Windows" => false,
                    "Linux" => true,
                    _ => return Err(Refused::Malformed),
                };
                Ok(Received::About(About {
                    program: program.into(),
                    title: title.into(),
                    window: u64::from_str_radix(window, 16).map_err(|_| Refused::Malformed)?,
                    name: name.into(),
                    pid: pid.parse().map_err(|_| Refused::Malformed)?,
                    linux,
                    command_line: command_line.into(),
                    text: text.into(),
                }))
            }
            Some(&MAGIC_FOLDER) => {
                let [_, program, title, window, path] = f[..] else {
                    return Err(Refused::Malformed);
                };
                Ok(Received::Folder(Folder {
                    program: program.into(),
                    title: title.into(),
                    window: u64::from_str_radix(window, 16).map_err(|_| Refused::Malformed)?,
                    path: path.into(),
                }))
            }
            _ => Err(Refused::Malformed),
        }
    }

    /// The wire form: UTF-8, fields separated by NUL (no field holds
    /// one; a NUL typed into a field is dropped): the magic, program,
    /// title, window (hex), text, then the element's id, label and
    /// rectangle (x, y, w, h in decimal), empty when there is none.
    pub fn encode(h: &Handoff) -> Vec<u8> {
        let window = alloc::format!("{:x}", h.window);
        let (id, label, rect) = match &h.element {
            Some(e) => {
                let [x, y, w, hh] = e.rect;
                (e.id.as_str(), e.label.as_str(), alloc::format!("{x},{y},{w},{hh}"))
            }
            None => ("", "", String::new()),
        };
        fields(&[MAGIC, &h.program, &h.title, &window, &h.text, id, label, &rect])
    }

    /// A request (`gala-handoff 1`); a process's form is Malformed here.
    pub fn decode(bytes: &[u8]) -> Result<Handoff, Refused> {
        let f = split(bytes)?;
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
    /// A program handed a process over to be asked about (WM_COPYDATA).
    About(handoff::About),
    /// A window's folder (WM_COPYDATA, File Explorer's).
    Folder(handoff::Folder),
    /// The model's reply to request `.0` (the program's worker thread).
    Answer(usize, Reply),
    /// The user clicked step `.1` of request `.0`'s Run, or Cancel.
    Run(usize, usize),
    Cancel(usize, usize),
    /// The user clicked step `.1` of request `.0`'s line: its details
    /// open, or close.
    Toggle(usize, usize),
    /// The command of step `.1` of request `.0` ended (the program's
    /// worker thread).
    Ran(usize, usize, Ran),
}

#[derive(Default)]
pub struct Panel {
    pub hidden: bool,
    /// None until a window other than the panel is active.
    pub context: Option<Context>,
    pub draft: String,
    pub requests: Vec<Request>,
    /// The process the panel is about (Task Manager's "Ask the agent"),
    /// until a window of another program is active: the chip names it
    /// and the model is told about it (`chat::body`).
    pub about: Option<handoff::About>,
    /// The folders windows reported (`handoff::Folder`), the latest of
    /// at most `FOLDERS` windows.
    pub folders: Vec<handoff::Folder>,
    /// What the machine has to run with (the program finds it at start).
    pub tools: Tools,
    /// What the program is to do (`take_due`), oldest first.
    pub due: Vec<Due>,
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

/// A request's answer: "Reply{i}", named with what it says. The
/// agent's words are set on the panel, the user's in a bubble
/// (`request`), so who said what is plain.
fn reply(i: usize, r: &Request) -> Node<Msg> {
    let calls = || r.rounds.iter().flat_map(|round| &round.calls);
    let (t, color) = match &r.answer {
        Some(Answer::Said(t)) => (t.clone(), INK),
        Some(Answer::Failed(e)) => (alloc::format!("No answer: {e}"), FAILED),
        _ if calls().any(|c| c.step == Step::Waiting) => {
            (String::from("Run the command, or cancel it."), MUTED)
        }
        _ if calls().any(|c| c.step == Step::Running) => {
            (String::from("Running the command..."), MUTED)
        }
        _ => (String::from("Asking the model..."), MUTED),
    };
    text(t).id(alloc::format!("Reply{i}")).size(13).color(color)
}

/// What step `c` came to, in a word or two for its line, and the colour.
fn badge(c: &Call) -> (&'static str, u32) {
    match &c.step {
        Step::Waiting => ("", MUTED),
        Step::Running => ("Running...", MUTED),
        Step::Declined => ("Cancelled", MUTED),
        Step::Refused(_) => ("Not run", FAILED),
        Step::Ran(Ran { code: None, .. }) => ("Did not start", FAILED),
        Step::Ran(Ran { code: Some(0), .. }) => ("Done", OK),
        Step::Ran(Ran { code: Some(_), .. }) => ("Failed", FAILED),
    }
}

/// Step `n` of request `i`: "Request{i}Step{n}". Waiting, it shows the
/// command, the folder and the explanation with Run and Cancel, so what
/// is run is seen before it is. Otherwise it is a line
/// ("Request{i}Step{n}Toggle": its command, what it came to, Show) that
/// opens on a click to the same details and where it stands
/// ("Request{i}Step{n}Result").
fn step(i: usize, n: usize, c: &Call, work: &Work) -> Node<Msg> {
    let id = |part: &str| alloc::format!("Request{i}Step{n}{part}");
    let folder = match work {
        Work::Windows { folder, .. } => alloc::format!("In {folder}"),
        Work::Linux { distro, path, .. } => alloc::format!("In {path} ({distro})"),
        Work::Unknown(why) => alloc::format!("No folder: {why}"),
    };
    let command = if c.command.is_empty() { c.name.clone() } else { c.command.clone() };
    let (status, color) = match &c.step {
        Step::Waiting => (String::new(), MUTED),
        Step::Running => (String::from("Running..."), MUTED),
        Step::Declined => (String::from("Cancelled: not run."), MUTED),
        Step::Refused(why) => (alloc::format!("Not run: {why}."), FAILED),
        Step::Ran(Ran { code: None, output, .. }) => {
            (alloc::format!("Could not run it: {output}"), FAILED)
        }
        Step::Ran(Ran { code: Some(code), output, bytes }) => {
            let mut t = alloc::format!("Exit code {code}");
            if *bytes == 0 {
                t.push_str(", no output.");
            } else {
                t.push_str(":\n");
                let shown: String = output.chars().take(SHOWN).collect();
                t.push_str(shown.trim_end());
                if shown.len() < *bytes {
                    t.push_str(&alloc::format!("\n({bytes} bytes in all)"));
                }
            }
            (t, if *code == 0 { INK } else { FAILED })
        }
    };
    let waiting = c.step == Step::Waiting;
    let open = waiting || c.step == Step::Running || c.open;
    // The command is the line's own text once there is a line (cut to
    // one line while closed, whole when open), so details give it only
    // while the step waits.
    let details = column()
        .gap(4)
        .child_if(waiting, text(command.clone()).id(id("Command")).size(12).color(INK))
        .child(text(folder).id(id("Folder")).size(12).color(MUTED))
        .child_if(
            !c.explanation.is_empty(),
            text(c.explanation.clone()).id(id("Explanation")).size(12).color(MUTED),
        );
    let card = column().id(id("")).gap(6).padding(8).radius(6).bg(CARD).border(LINE);
    if waiting {
        return card
            .child(text("Run this command?").size(12).bold().color(MUTED))
            .child(details)
            .child(
                row()
                    .gap(8)
                    .child(
                        button(id("Run"), "Run")
                            .width(64)
                            .height(28)
                            .bg(ACCENT)
                            .color(0xffffff)
                            .on_click(Msg::Run(i, n)),
                    )
                    .child(
                        button(id("Cancel"), "Cancel")
                            .width(64)
                            .height(28)
                            .on_click(Msg::Cancel(i, n)),
                    ),
            );
    }
    let (word, tint) = badge(c);
    let line = row()
        .id(id("Toggle"))
        .gap(8)
        .align(Align::Center)
        .on_click(Msg::Toggle(i, n))
        .child({
            let summary = text(command).id(id("Summary")).size(12).bold().grow(1);
            let summary = if open { summary } else { summary.ellipsis() };
            summary.on_click(Msg::Toggle(i, n))
        })
        .child(text(word).size(12).color(tint))
        .child_if(
            c.step != Step::Running,
            text(if c.open { "Hide" } else { "Show" }).size(12).color(ACCENT),
        );
    card.child(line)
        .child_if(open, details)
        .child_if(open, text(status).id(id("Result")).size(12).color(color))
}

/// Request `i`: the program it was for, what the user asked (a bubble
/// on the right, "Request{i}Text"), then the agent's side: what it said
/// beside its calls, its steps and its answer.
fn request(i: usize, r: &Request) -> Node<Msg> {
    let mut agent = column().gap(6);
    let mut n = 0;
    for round in &r.rounds {
        if let Some(t) = round.text.as_ref().filter(|t| !t.trim().is_empty()) {
            agent = agent.child(text(t.clone()).size(13).color(INK));
        }
        for c in &round.calls {
            agent = agent.child(step(i, n, c, &r.work));
            n += 1;
        }
    }
    column()
        .id(alloc::format!("Request{i}"))
        .gap(6)
        .child(
            column()
                .align(Align::End)
                .gap(2)
                .child_if(!r.program.is_empty(), text(r.program.clone()).size(11).color(MUTED))
                .child_if(
                    r.element.is_some(),
                    text(alloc::format!("On: {}", r.element.as_deref().unwrap_or("")))
                        .id(alloc::format!("Request{i}Element"))
                        .size(11)
                        .color(MUTED),
                )
                .child(
                    column().max_width(256).padding((8, 12)).radius(12).bg(ACCENT).child(
                        text(r.text.clone())
                            .id(alloc::format!("Request{i}Text"))
                            .size(14)
                            .color(0xffffff),
                    ),
                ),
        )
        .child(agent.child_if(r.answer.is_some(), reply(i, r)))
}

/// The working folder for a window of program `c`, from the folder it
/// reported.
fn work_of(c: Option<&Context>, folders: &[handoff::Folder]) -> Work {
    let Some(c) = c else {
        return Work::Unknown(String::from("no window is active"));
    };
    let Some(f) = folders.iter().find(|f| c.window != 0 && f.window == c.window) else {
        return Work::Unknown(alloc::format!("{} reports no folder", c.program));
    };
    if f.path.is_empty() {
        return Work::Unknown(alloc::format!("{} shows no file-system folder", c.program));
    }
    let program = c.program.clone();
    for prefix in ["\\\\wsl.localhost\\", "\\\\wsl$\\"] {
        let head = f.path.get(..prefix.len());
        if head.is_some_and(|h| h.eq_ignore_ascii_case(prefix)) {
            let rest = &f.path[prefix.len()..];
            let (distro, tail) = rest.split_once('\\').unwrap_or((rest, ""));
            let path = alloc::format!("/{}", tail.replace('\\', "/"));
            return Work::Linux { program, distro: distro.into(), path };
        }
    }
    Work::Windows { program, folder: f.path.clone() }
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
        let chip = match (&self.about, &self.context) {
            (Some(a), _) => row()
                .id("ContextChip")
                .gap(6)
                .padding((6, 12))
                .radius(14)
                .bg(CHIP)
                .align(Align::Center)
                .child(text(a.name.clone()).id("ContextProgram").bold().size(13))
                .child(
                    text(alloc::format!("pid {}, {}", a.pid, a.personality()))
                        .id("ContextTitle")
                        .size(13)
                        .color(MUTED)
                        .ellipsis(),
                ),
            (None, Some(c)) => row()
                .id("ContextChip")
                .gap(6)
                .padding((6, 12))
                .radius(14)
                .bg(CHIP)
                .align(Align::Center)
                .child(text(c.program.clone()).id("ContextProgram").bold().size(13))
                .child(text(c.title.clone()).id("ContextTitle").size(13).color(MUTED).ellipsis()),
            (None, None) => row()
                .id("ContextChip")
                .padding((6, 12))
                .radius(14)
                .bg(CHIP)
                .child(text("No window is active").id("ContextProgram").size(13).color(MUTED)),
        };
        let requests: Vec<Node<Msg>> =
            self.requests.iter().enumerate().map(|(i, r)| request(i, r)).collect();
        let empty = self.requests.is_empty();
        let folder = match (&self.about, work_of(self.context.as_ref(), &self.folders)) {
            (None, Work::Windows { folder, .. }) => Some(folder),
            (None, Work::Linux { distro, path, .. }) => Some(alloc::format!("{path} ({distro})")),
            _ => None,
        };
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
            .child(
                column().gap(4).padding((0, 12)).child(chip).child_if(
                    folder.is_some(),
                    text(folder.unwrap_or_default())
                        .id("ContextFolder")
                        .size(12)
                        .color(MUTED)
                        .ellipsis(),
                ),
            )
            .child(
                column()
                    .id("Requests")
                    .grow(1)
                    .gap(16)
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
                            .padding((0, 8))
                            .radius(4)
                            .bg(CARD)
                            .border(INPUT_LINE)
                            .on_input(Msg::Draft),
                    )
                    .child_if(
                        !self.draft.is_empty() && !self.busy(),
                        button("SendButton", "Send")
                            .width(56)
                            .height(32)
                            .bg(ACCENT)
                            .color(0xffffff)
                            .on_click(Msg::Send),
                    ),
            )
    }

    /// Whether a request's loop is unfinished (the model being called,
    /// a step waiting or running): no other is sent meanwhile.
    pub fn busy(&self) -> bool {
        self.requests.iter().any(|r| r.answer == Some(Answer::Asking))
    }

    /// What the program is to do next, oldest first: call the model (the
    /// body built now, from the conversation so far), or run a command.
    pub fn take_due(&mut self) -> Option<Due> {
        if self.due.is_empty() {
            return None;
        }
        let mut d = self.due.remove(0);
        if let Due::Ask { index, round, body } = &mut d {
            if let Some(r) = self.requests.get_mut(*index) {
                r.asked += 1;
                *round = r.asked;
            }
            *body =
                chat::body(self.context.as_ref(), self.about.as_ref(), &self.requests, &self.tools);
        }
        Some(d)
    }

    /// The call `n` of request `i`, counting through its rounds.
    fn call_mut(&mut self, i: usize, n: usize) -> Option<&mut Call> {
        self.requests.get_mut(i)?.rounds.iter_mut().flat_map(|r| &mut r.calls).nth(n)
    }

    /// Once request `i`'s last round has every call's result: the model
    /// is asked again with them, unless it was called `ROUNDS` times.
    fn after_steps(&mut self, i: usize) {
        let Some(r) = self.requests.get_mut(i) else { return };
        let done = r.rounds.last().is_none_or(|round| {
            round.calls.iter().all(|c| !matches!(c.step, Step::Waiting | Step::Running))
        });
        if !done || r.answer != Some(Answer::Asking) {
            return;
        }
        if r.asked >= ROUNDS {
            r.answer = Some(Answer::Failed(alloc::format!(
                "stopped after {ROUNDS} rounds without a final answer"
            )));
        } else {
            self.due.push(Due::Ask { index: i, round: 0, body: String::new() });
        }
    }

    /// The calls of a reply to request `i`, as steps: each `run` with
    /// a command waits for the user, where there is a folder to run in;
    /// any other is refused, with why (the model is told).
    fn calls(&mut self, i: usize, text: Option<String>, calls: Vec<chat::ToolCall>) {
        let tools = self.tools.clone();
        let Some(r) = self.requests.get_mut(i) else { return };
        let refusal = match (&r.work, &tools.cmd) {
            (Work::Windows { .. }, Some(_)) => None,
            (Work::Windows { .. }, None) => Some(String::from("cmd.exe is not installed")),
            (Work::Linux { distro, .. }, _) => Some(alloc::format!(
                "running in the Linux distribution {distro} is not supported yet"
            )),
            (Work::Unknown(why), _) => Some(alloc::format!("no working folder is known ({why})")),
        };
        let calls = calls
            .into_iter()
            .map(|c| {
                let parsed = if c.name == chat::RUN {
                    chat::run_arguments(&c.arguments)
                } else {
                    Err(alloc::format!("there is no tool named {:?}", c.name))
                };
                let (command, explanation, step) = match parsed {
                    Ok((command, explanation)) => {
                        let step = refusal.clone().map_or(Step::Waiting, Step::Refused);
                        (command, explanation, step)
                    }
                    Err(why) => (String::new(), String::new(), Step::Refused(why)),
                };
                Call {
                    id: c.id,
                    name: c.name,
                    arguments: c.arguments,
                    command,
                    explanation,
                    step,
                    open: false,
                }
            })
            .collect();
        r.rounds.push(Round { text, calls });
        self.after_steps(i);
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Hide => self.hidden = true,
            Msg::Show => self.hidden = false,
            Msg::Draft(t) => self.draft = t,
            Msg::Send => {
                if self.draft.trim().is_empty() || self.busy() {
                    return;
                }
                let text = core::mem::take(&mut self.draft);
                let program = self.context.as_ref().map(|c| c.program.clone()).unwrap_or_default();
                let work = match &self.about {
                    Some(a) => Work::Unknown(alloc::format!("{} shows no folder", a.program)),
                    None => work_of(self.context.as_ref(), &self.folders),
                };
                let mut r = Request::new(program, text, None, work);
                r.answer = Some(Answer::Asking);
                self.requests.push(r);
                let index = self.requests.len() - 1;
                self.due.push(Due::Ask { index, round: 0, body: String::new() });
            }
            Msg::Answer(i, reply) => {
                // Only a request whose model is being called takes one.
                let Some(r) = self.requests.get_mut(i) else { return };
                let waiting = r.rounds.last().is_some_and(|round| {
                    round.calls.iter().any(|c| matches!(c.step, Step::Waiting | Step::Running))
                });
                if r.answer != Some(Answer::Asking) || waiting {
                    return;
                }
                match reply {
                    Ok(chat::Said::Text(t)) => r.answer = Some(Answer::Said(t)),
                    Ok(chat::Said::Calls { text, calls }) => self.calls(i, text, calls),
                    Err(e) => r.answer = Some(Answer::Failed(e)),
                }
            }
            Msg::Run(i, n) => {
                let work = self.requests.get(i).map(|r| r.work.clone());
                let cmd = self.tools.cmd.clone();
                let Some(c) = self.call_mut(i, n) else { return };
                if c.step != Step::Waiting {
                    return;
                }
                let (Some(Work::Windows { folder, .. }), Some(cmd)) = (work, cmd) else { return };
                c.step = Step::Running;
                let command = c.command.clone();
                self.due.push(Due::Run { index: i, step: n, cmd, command, folder });
            }
            Msg::Cancel(i, n) => {
                let Some(c) = self.call_mut(i, n) else { return };
                if c.step != Step::Waiting {
                    return;
                }
                c.step = Step::Declined;
                self.after_steps(i);
            }
            Msg::Toggle(i, n) => {
                if let Some(c) = self.call_mut(i, n) {
                    c.open = !c.open;
                }
            }
            Msg::Ran(i, n, ran) => {
                let Some(c) = self.call_mut(i, n) else { return };
                if c.step != Step::Running {
                    return;
                }
                c.step = Step::Ran(ran);
                self.after_steps(i);
            }
            Msg::Context(c) => self.set_context(c),
            Msg::Folder(f) => {
                self.folders.retain(|g| g.window != f.window);
                if self.folders.len() >= FOLDERS {
                    self.folders.remove(0);
                }
                self.folders.push(f);
            }
            Msg::Handoff(h) => {
                self.hidden = false;
                self.set_context(Context {
                    program: h.program.clone(),
                    title: h.title,
                    window: h.window,
                });
                let work = work_of(self.context.as_ref(), &self.folders);
                self.requests.push(Request::new(
                    h.program,
                    h.text,
                    h.element.map(|e| e.label),
                    work,
                ));
            }
            Msg::About(a) => {
                self.hidden = false;
                self.context = Some(Context {
                    program: a.program.clone(),
                    title: a.title.clone(),
                    window: a.window,
                });
                let work = Work::Unknown(alloc::format!("{} shows no folder", a.program));
                let text = alloc::format!("About: {} (pid {})", a.name, a.pid);
                self.requests.push(Request::new(a.program.clone(), text, None, work));
                self.about = Some(a);
            }
        }
    }

    /// The active window's program and title. A process handed over
    /// stays what the panel is about while its sender's windows are
    /// active (Task Manager's, the panel's own never being a context)
    /// and is let go when another program's is.
    fn set_context(&mut self, c: Context) {
        if self.about.as_ref().is_some_and(|a| a.program != c.program) {
            self.about = None;
        }
        self.context = Some(c);
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

    /// The panel laid out once step `n` of request `i` is opened, by a
    /// click on its line as the user would.
    fn opened(p: &mut Panel, i: usize, n: usize) -> Vec<Placed<Msg>> {
        let placed = lay(p, WIDTH, 752);
        let line = alloc::format!("Request{i}Step{n}Toggle");
        let (x, y) = centre(at(&placed, &line));
        let (_, msg) = gala_ui::click(&placed, x, y).unwrap();
        assert_eq!(msg, Msg::Toggle(i, n));
        p.update(msg);
        lay(p, WIDTH, 752)
    }

    /// The model's call that is due, as the program would make it: the
    /// request's index and the body.
    fn asking(p: &mut Panel) -> Option<(usize, String)> {
        match p.take_due()? {
            Due::Ask { index, body, .. } => Some((index, body)),
            d => panic!("not a call of the model: {d:?}"),
        }
    }

    fn said(t: &str) -> Reply {
        Ok(chat::Said::Text(t.into()))
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
        p.update(Msg::Context(Context {
            program: "File Explorer".into(),
            title: "tmp".into(),
            window: 0,
        }));
        let placed = lay(&p, WIDTH, 800);
        assert_eq!(at(&placed, "ContextProgram").text, "File Explorer");
        assert_eq!(at(&placed, "ContextTitle").text, "tmp");
        assert!(placed.iter().all(|e| e.id.as_deref() != Some("SendButton")));
        p.update(Msg::Draft("compress all the files".into()));
        let placed = lay(&p, WIDTH, 800);
        let (x, y) = centre(at(&placed, "SendButton"));
        assert_eq!(gala_ui::click(&placed, x, y), Some(("SendButton", Msg::Send)));
        p.update(Msg::Send);
        let mut asked = Request::new(
            "File Explorer".into(),
            "compress all the files".into(),
            None,
            Work::Unknown("File Explorer reports no folder".into()),
        );
        asked.answer = Some(Answer::Asking);
        assert_eq!(p.requests, [asked]);
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

    fn process() -> handoff::About {
        handoff::About {
            program: "Task Manager".into(),
            title: "Task Manager".into(),
            window: 0x20030,
            name: "sleep".into(),
            pid: 4108,
            linux: true,
            command_line: "sleep 1000".into(),
            text: "Name: sleep\nPID: 4108\nCPU over the last 3 s: 0.0, 0.1, 0.0".into(),
        }
    }

    #[test]
    fn a_request_keeps_its_bytes() {
        // Explorer's form, byte for byte as before processes could be
        // handed over (agent_test sends these same bytes).
        assert_eq!(
            handoff::encode(&sample()),
            b"gala-handoff 1\0File Explorer\0tmp\x0020010\0bigger, with\ntwo lines\0\
              AddressBar\0the address bar\x0064,-2,600,32\0"
        );
        let none = handoff::Handoff { element: None, ..sample() };
        assert_eq!(
            handoff::encode(&none),
            b"gala-handoff 1\0File Explorer\0tmp\x0020010\0bigger, with\ntwo lines\0\0\0\0"
        );
        assert_eq!(handoff::receive(&handoff::encode(&none)), Ok(handoff::Received::Request(none)));
    }

    #[test]
    fn a_process_round_trips_and_refuses() {
        use handoff::{Received, Refused, decode, encode_about, receive};
        let a = process();
        let bytes = encode_about(&a);
        assert_eq!(
            bytes,
            b"gala-process 1\0Task Manager\0Task Manager\x0020030\0sleep\x004108\0Linux\0\
              sleep 1000\0Name: sleep\nPID: 4108\nCPU over the last 3 s: 0.0, 0.1, 0.0\0"
        );
        assert_eq!(receive(&bytes), Ok(Received::About(a.clone())));
        let windows = handoff::About { linux: false, command_line: String::new(), ..a.clone() };
        assert_eq!(receive(&encode_about(&windows)), Ok(Received::About(windows)));
        // A NUL in the text cannot split it.
        let nul = handoff::About { text: "a\0b".into(), ..a.clone() };
        assert_eq!(
            receive(&encode_about(&nul)),
            Ok(Received::About(handoff::About { text: "ab".into(), ..a.clone() }))
        );
        // Only `receive` takes it: a panel that knows requests alone
        // (`decode`) refuses it.
        assert_eq!(decode(&bytes), Err(Refused::Malformed));
        let refused = |b: &[u8]| receive(b).err();
        assert_eq!(
            refused(b"gala-process 2\0a\0b\x001\0n\x001\0Windows\0\0t\0"),
            Some(Refused::Malformed)
        );
        assert_eq!(
            refused(b"gala-process 1\0a\0b\x001\0n\x001\0Mac\0\0t\0"),
            Some(Refused::Malformed)
        );
        assert_eq!(
            refused(b"gala-process 1\0a\0b\x001\0n\0-1\0Linux\0\0t\0"),
            Some(Refused::Malformed)
        );
        assert_eq!(
            refused(b"gala-process 1\0a\0b\0zz\0n\x001\0Linux\0\0t\0"),
            Some(Refused::Malformed)
        );
        assert_eq!(
            refused(b"gala-process 1\0a\0b\x001\0n\x001\0Linux\0\0"),
            Some(Refused::Malformed)
        );
        assert_eq!(
            refused(b"gala-process 1\0a\0b\x001\0n\x001\0Linux\0\0t"),
            Some(Refused::Malformed)
        );
        assert_eq!(refused(b"gala-handoff 1\0short\0"), Some(Refused::Malformed));
        assert_eq!(refused(b""), Some(Refused::Malformed));
        assert_eq!(refused(&[0xff, 0]), Some(Refused::NotUtf8));
        let long = handoff::About { text: "x".repeat(handoff::MAX), ..a };
        assert_eq!(refused(&encode_about(&long)), Some(Refused::TooLong));
    }

    #[test]
    fn a_process_docks_and_is_what_the_panel_is_about() {
        let mut p = Panel { hidden: true, ..Panel::default() };
        p.update(Msg::About(process()));
        assert!(!p.hidden);
        let placed = lay(&p, WIDTH, 752);
        assert_eq!(at(&placed, "ContextProgram").text, "sleep");
        assert_eq!(at(&placed, "ContextTitle").text, "pid 4108, Linux");
        assert_eq!(at(&placed, "Request0Text").text, "About: sleep (pid 4108)");
        assert!(placed.iter().all(|e| e.id.as_deref() != Some("Request0Element")));
        // Listed, not asked.
        assert_eq!(asking(&mut p), None);
        assert!(placed.iter().all(|e| e.id.as_deref() != Some("Reply0")));
        // A question then goes to the model with the process's text.
        p.update(Msg::Draft("why is it busy?".into()));
        p.update(Msg::Send);
        let (i, body) = asking(&mut p).unwrap();
        assert_eq!(i, 1);
        assert!(body.contains("as Task Manager shows it:\\nName: sleep\\nPID: 4108"), "{body}");
        assert!(
            body.contains("{\"role\":\"user\",\"content\":\"why is it busy?\"}],\"tools\""),
            "{body}"
        );
        // While it is being asked no other is sent.
        p.update(Msg::Draft("another".into()));
        p.update(Msg::Send);
        assert_eq!(p.requests.len(), 2);
        assert!(lay(&p, WIDTH, 752).iter().all(|e| e.id.as_deref() != Some("SendButton")));
        p.update(Msg::Answer(1, said("It waits.")));
        // Task Manager active again: still about the process.
        p.update(Msg::Context(Context {
            program: "Task Manager".into(),
            title: "Task Manager".into(),
            window: 0,
        }));
        assert_eq!(at(&lay(&p, WIDTH, 752), "ContextProgram").text, "sleep");
        // Another program active: the chip names it, the process is let
        // go (its card stays), and the model is no longer told of it.
        p.update(Msg::Context(Context {
            program: "Notepad".into(),
            title: "a.txt".into(),
            window: 0,
        }));
        let placed = lay(&p, WIDTH, 752);
        assert_eq!(at(&placed, "ContextProgram").text, "Notepad");
        assert_eq!(at(&placed, "ContextTitle").text, "a.txt");
        assert_eq!(at(&placed, "Request0Text").text, "About: sleep (pid 4108)");
        assert_eq!(p.about, None);
        p.update(Msg::Draft("and now?".into()));
        p.update(Msg::Send);
        assert!(!asking(&mut p).unwrap().1.contains("asks about the process"));
        // A request handed over by another program lets it go too.
        p.update(Msg::About(process()));
        p.update(Msg::Handoff(sample()));
        assert_eq!(p.about, None);
        assert_eq!(at(&lay(&p, WIDTH, 752), "ContextProgram").text, "File Explorer");
    }

    #[test]
    fn handoff_docks_and_lists_the_request() {
        let mut p = Panel { hidden: true, ..Panel::default() };
        p.update(Msg::Handoff(sample()));
        assert!(!p.hidden);
        assert_eq!(
            p.context,
            Some(Context { program: "File Explorer".into(), title: "tmp".into(), window: 0x20010 })
        );
        let placed = lay(&p, WIDTH, 752);
        assert_eq!(at(&placed, "Request0Text").text, "bigger, with\ntwo lines");
        assert_eq!(at(&placed, "Request0Element").text, "On: the address bar");
        assert_eq!(at(&placed, "ContextProgram").text, "File Explorer");
    }

    #[test]
    fn a_sent_request_is_asked_and_answered() {
        let mut p = Panel::default();
        p.update(Msg::Context(Context {
            program: "Notepad".into(),
            title: "a.txt".into(),
            window: 0,
        }));
        assert_eq!(asking(&mut p), None);
        p.update(Msg::Draft("say pong".into()));
        p.update(Msg::Send);
        let (i, body) = asking(&mut p).unwrap();
        assert_eq!(i, 0);
        assert!(body.contains("{\"role\":\"user\",\"content\":\"say pong\"}],\"tools\""), "{body}");
        assert_eq!(at(&lay(&p, WIDTH, 752), "Reply0").text, "Asking the model...");
        p.update(Msg::Answer(0, said("Pong")));
        assert_eq!(asking(&mut p), None);
        assert_eq!(at(&lay(&p, WIDTH, 752), "Reply0").text, "Pong");
        // A second answer to it, or one to no request, changes nothing.
        p.update(Msg::Answer(0, said("again")));
        p.update(Msg::Answer(7, said("none")));
        assert_eq!(p.requests[0].answer, Some(Answer::Said("Pong".into())));
        // The next is asked with the conversation so far.
        p.update(Msg::Draft("and?".into()));
        p.update(Msg::Send);
        let (i, body) = asking(&mut p).unwrap();
        assert_eq!(i, 1);
        assert!(body.contains("\"assistant\",\"content\":\"Pong\""), "{body}");
        p.update(Msg::Answer(1, Err("agent-relay: not recorded".into())));
        let placed = lay(&p, WIDTH, 752);
        assert_eq!(at(&placed, "Reply1").text, "No answer: agent-relay: not recorded");
        // A hand-off is listed, not asked.
        p.update(Msg::Handoff(sample()));
        assert_eq!(asking(&mut p), None);
        assert!(lay(&p, WIDTH, 752).iter().all(|e| e.id.as_deref() != Some("Reply2")));
    }

    fn explorer(path: &str) -> handoff::Folder {
        handoff::Folder {
            program: "File Explorer".into(),
            title: "agent_run".into(),
            window: 0x20040,
            path: path.into(),
        }
    }

    const FOLDER: &str = "C:\\Users\\gala\\AppData\\Local\\Temp\\agent_run";

    #[test]
    fn a_folder_round_trips_and_refuses() {
        use handoff::{Received, Refused, encode_folder, receive};
        let f = explorer(FOLDER);
        let bytes = encode_folder(&f);
        assert_eq!(
            bytes,
            b"gala-folder 1\0File Explorer\0agent_run\x0020040\0\
              C:\\Users\\gala\\AppData\\Local\\Temp\\agent_run\0"
        );
        assert_eq!(receive(&bytes), Ok(Received::Folder(f.clone())));
        let none = explorer("");
        assert_eq!(receive(&encode_folder(&none)), Ok(Received::Folder(none)));
        assert_eq!(receive(b"gala-folder 1\0a\0b\x001\0"), Err(Refused::Malformed));
        assert_eq!(receive(b"gala-folder 1\0a\0b\0zz\0p\0"), Err(Refused::Malformed));
        assert_eq!(receive(b"gala-folder 1\0a\0b\x001\0p\0x\0"), Err(Refused::Malformed));
        assert_eq!(handoff::decode(&bytes), Err(Refused::Malformed));
    }

    /// A panel on File Explorer's window showing `path`, with cmd.exe
    /// and busybox.
    fn on_explorer(path: &str) -> Panel {
        let mut p = Panel {
            tools: Tools { cmd: Some("C:\\Windows\\System32\\cmd.exe".into()), busybox: true },
            ..Panel::default()
        };
        p.update(Msg::Context(Context {
            program: "File Explorer".into(),
            title: "agent_run".into(),
            window: 0x20040,
        }));
        p.update(Msg::Folder(explorer(path)));
        p
    }

    fn calls(calls: &[(&str, &str, &str)]) -> Reply {
        Ok(chat::Said::Calls {
            text: None,
            calls: calls
                .iter()
                .map(|&(id, name, arguments)| chat::ToolCall {
                    id: id.into(),
                    name: name.into(),
                    arguments: arguments.into(),
                })
                .collect(),
        })
    }

    const TAR: &str = "{\"command\":\"busybox tar -czf files.tar.gz one.txt two.txt\",\
                       \"explanation\":\"Packs the files into files.tar.gz.\"}";

    #[test]
    fn the_folder_is_the_working_folder() {
        let mut p = on_explorer(FOLDER);
        assert_eq!(at(&lay(&p, WIDTH, 752), "ContextFolder").text, FOLDER);
        p.update(Msg::Draft("compress all the files".into()));
        p.update(Msg::Send);
        let work = Work::Windows { program: "File Explorer".into(), folder: FOLDER.into() };
        assert_eq!(p.requests[0].work, work);
        let (_, body) = asking(&mut p).unwrap();
        assert!(body.contains("in the folder File Explorer shows: C:\\\\Users\\\\gala"), "{body}");
        // Another window of it, with no folder reported: none known.
        p.update(Msg::Context(Context { window: 0x20050, ..p.context.clone().unwrap() }));
        assert!(lay(&p, WIDTH, 752).iter().all(|e| e.id.as_deref() != Some("ContextFolder")));
        assert_eq!(
            work_of(p.context.as_ref(), &p.folders),
            Work::Unknown("File Explorer reports no folder".into())
        );
        // A newer report of a window replaces its older; at most FOLDERS.
        for w in 0..FOLDERS as u64 + 4 {
            p.update(Msg::Folder(handoff::Folder { window: w, ..explorer("C:\\") }));
        }
        p.update(Msg::Folder(handoff::Folder { window: 5, ..explorer("D:\\") }));
        assert_eq!(p.folders.len(), FOLDERS);
        assert_eq!(p.folders.iter().filter(|f| f.window == 5).count(), 1);
        assert_eq!(p.folders.last().map(|f| f.path.as_str()), Some("D:\\"));
        // Home (no file-system folder), and a distribution's folder.
        let p = on_explorer("");
        assert_eq!(
            work_of(p.context.as_ref(), &p.folders),
            Work::Unknown("File Explorer shows no file-system folder".into())
        );
        let p = on_explorer("\\\\wsl.localhost\\Alpine\\home\\gala");
        assert_eq!(
            work_of(p.context.as_ref(), &p.folders),
            Work::Linux {
                program: "File Explorer".into(),
                distro: "Alpine".into(),
                path: "/home/gala".into()
            }
        );
        assert_eq!(at(&lay(&p, WIDTH, 752), "ContextFolder").text, "/home/gala (Alpine)");
        assert_eq!(work_of(None, &[]), Work::Unknown("no window is active".into()));
    }

    #[test]
    fn a_call_waits_for_run_and_its_result_goes_back() {
        let mut p = on_explorer(FOLDER);
        p.update(Msg::Draft("compress all the files".into()));
        p.update(Msg::Send);
        assert_eq!(asking(&mut p).map(|a| a.0), Some(0));
        assert_eq!(p.take_due(), None);
        // What the user asked is a bubble on the right, apart from the
        // agent's side; the prompt is a framed box.
        let placed = lay(&p, WIDTH, 752);
        let asked = at(&placed, "Request0Text").rect;
        assert!(asked[0] > WIDTH / 3 && asked[0] + asked[2] >= WIDTH - 32, "{asked:?}");
        assert_eq!(at(&placed, "Prompt").border, Some(INPUT_LINE));
        assert_eq!(at(&placed, "Prompt").background, Some(CARD));
        p.update(Msg::Answer(0, calls(&[("call_1", "run", TAR)])));
        // Shown with Run and Cancel; nothing is due until Run.
        assert_eq!(p.take_due(), None);
        let placed = lay(&p, WIDTH, 752);
        assert_eq!(
            at(&placed, "Request0Step0Command").text,
            "busybox tar -czf files.tar.gz one.txt two.txt"
        );
        assert_eq!(at(&placed, "Request0Step0Folder").text, alloc::format!("In {FOLDER}"));
        assert_eq!(
            at(&placed, "Request0Step0Explanation").text,
            "Packs the files into files.tar.gz."
        );
        assert_eq!(at(&placed, "Reply0").text, "Run the command, or cancel it.");
        assert!(placed.iter().all(|e| e.id.as_deref() != Some("Request0Step0Result")));
        let (x, y) = centre(at(&placed, "Request0Step0Run"));
        assert_eq!(gala_ui::click(&placed, x, y), Some(("Request0Step0Run", Msg::Run(0, 0))));
        let (x, y) = centre(at(&placed, "Request0Step0Cancel"));
        assert_eq!(gala_ui::click(&placed, x, y), Some(("Request0Step0Cancel", Msg::Cancel(0, 0))));
        // A reply while a step waits is not taken.
        p.update(Msg::Answer(0, said("early")));
        assert_eq!(p.requests[0].answer, Some(Answer::Asking));
        p.update(Msg::Run(0, 0));
        assert_eq!(
            p.take_due(),
            Some(Due::Run {
                index: 0,
                step: 0,
                cmd: "C:\\Windows\\System32\\cmd.exe".into(),
                command: "busybox tar -czf files.tar.gz one.txt two.txt".into(),
                folder: FOLDER.into(),
            })
        );
        // A second click, or Cancel now, does nothing.
        p.update(Msg::Run(0, 0));
        p.update(Msg::Cancel(0, 0));
        assert_eq!(p.take_due(), None);
        let placed = lay(&p, WIDTH, 752);
        assert_eq!(at(&placed, "Request0Step0Result").text, "Running...");
        assert_eq!(at(&placed, "Reply0").text, "Running the command...");
        p.update(Msg::Ran(0, 0, Ran { code: Some(0), output: String::new(), bytes: 0 }));
        // Ended, the step is a line: its command and what it came to,
        // the details shown only once it is opened, and closed again.
        let placed = lay(&p, WIDTH, 752);
        assert_eq!(
            at(&placed, "Request0Step0Summary").text,
            "busybox tar -czf files.tar.gz one.txt two.txt"
        );
        assert!(placed.iter().any(|e| e.text == "Done"));
        for hidden in ["Result", "Command", "Folder", "Explanation", "Run"] {
            let id = alloc::format!("Request0Step0{hidden}");
            assert!(placed.iter().all(|e| e.id.as_deref() != Some(id.as_str())), "{id}");
        }
        let placed = opened(&mut p, 0, 0);
        assert_eq!(at(&placed, "Request0Step0Result").text, "Exit code 0, no output.");
        assert_eq!(at(&placed, "Request0Step0Folder").text, alloc::format!("In {FOLDER}"));
        let placed = opened(&mut p, 0, 0);
        assert!(placed.iter().all(|e| e.id.as_deref() != Some("Request0Step0Result")));
        // The model is asked again, with the call and its result.
        let (i, body) = asking(&mut p).unwrap();
        assert_eq!((i, p.requests[0].asked), (0, 2));
        assert!(
            body.contains(
                "{\"role\":\"tool\",\"tool_call_id\":\"call_1\",\"content\":\"Exit code 0. No output.\"}"
            ),
            "{body}"
        );
        p.update(Msg::Answer(0, said("Done: files.tar.gz holds them.")));
        assert_eq!(at(&lay(&p, WIDTH, 752), "Reply0").text, "Done: files.tar.gz holds them.");
        assert!(!p.busy());
        // A result for no running step changes nothing.
        p.update(Msg::Ran(0, 0, Ran { code: Some(1), output: String::new(), bytes: 0 }));
        p.update(Msg::Ran(3, 0, Ran { code: Some(1), output: String::new(), bytes: 0 }));
        assert_eq!(p.take_due(), None);
    }

    #[test]
    fn output_is_shown_bounded() {
        let mut p = on_explorer(FOLDER);
        p.update(Msg::Draft("list".into()));
        p.update(Msg::Send);
        asking(&mut p);
        p.update(Msg::Answer(0, calls(&[("a", "run", "{\"command\":\"dir\"}")])));
        p.update(Msg::Run(0, 0));
        p.take_due();
        let output = "x".repeat(chat::MAX_OUTPUT);
        p.update(Msg::Ran(0, 0, Ran { code: Some(2), output, bytes: 20000 }));
        let shown = at(&opened(&mut p, 0, 0), "Request0Step0Result").text.clone();
        assert!(shown.starts_with("Exit code 2:\nxxx"), "{shown}");
        assert!(shown.ends_with("x\n(20000 bytes in all)"), "{shown}");
        // SHOWN of the output's, and Exit's.
        assert_eq!(shown.matches('x').count(), SHOWN + 1);
        // One that did not start.
        let mut p = on_explorer(FOLDER);
        p.update(Msg::Draft("list".into()));
        p.update(Msg::Send);
        asking(&mut p);
        p.update(Msg::Answer(0, calls(&[("a", "run", "{\"command\":\"dir\"}")])));
        p.update(Msg::Run(0, 0));
        p.take_due();
        let gone = Ran { code: None, output: "CreateProcessW failed: 2".into(), bytes: 0 };
        p.update(Msg::Ran(0, 0, gone));
        assert_eq!(
            at(&opened(&mut p, 0, 0), "Request0Step0Result").text,
            "Could not run it: CreateProcessW failed: 2"
        );
        assert!(asking(&mut p).unwrap().1.contains("Could not run it: CreateProcessW failed: 2."));
    }

    #[test]
    fn cancel_tells_the_model_and_bad_calls_are_refused() {
        let mut p = on_explorer(FOLDER);
        p.update(Msg::Draft("compress all the files".into()));
        p.update(Msg::Send);
        asking(&mut p);
        p.update(Msg::Answer(0, calls(&[("c1", "run", TAR)])));
        p.update(Msg::Cancel(0, 0));
        assert_eq!(at(&opened(&mut p, 0, 0), "Request0Step0Result").text, "Cancelled: not run.");
        let (_, body) = asking(&mut p).unwrap();
        assert!(body.contains("\"content\":\"The user declined to run this command.\""), "{body}");
        // A tool that is not run, and run without a command: refused, and
        // the model is asked again at once.
        p.update(Msg::Answer(0, calls(&[("c2", "shell", "{}"), ("c3", "run", "{}")])));
        opened(&mut p, 0, 1);
        let placed = opened(&mut p, 0, 2);
        assert_eq!(at(&placed, "Request0Step1Summary").text, "shell");
        assert_eq!(
            at(&placed, "Request0Step1Result").text,
            "Not run: there is no tool named \"shell\"."
        );
        assert_eq!(at(&placed, "Request0Step2Result").text, "Not run: it has no command string.");
        assert!(placed.iter().all(|e| e.id.as_deref() != Some("Request0Step1Run")));
        let (_, body) = asking(&mut p).unwrap();
        assert!(body.contains(
            "\"tool_call_id\":\"c3\",\"content\":\"Not run: it has no command string.\""
        ));
        assert_eq!(p.requests[0].asked, 3);
    }

    #[test]
    fn with_no_folder_every_run_is_refused() {
        for (path, why) in [
            ("", "no working folder is known (File Explorer shows no file-system folder)"),
            (
                "\\\\wsl.localhost\\Alpine\\home",
                "running in the Linux distribution Alpine is not supported yet",
            ),
        ] {
            let mut p = on_explorer(path);
            p.update(Msg::Draft("compress all the files".into()));
            p.update(Msg::Send);
            let (_, body) = asking(&mut p).unwrap();
            assert!(body.contains("every run is refused"), "{body}");
            p.update(Msg::Answer(0, calls(&[("c1", "run", TAR)])));
            let placed = opened(&mut p, 0, 0);
            assert!(placed.iter().all(|e| e.id.as_deref() != Some("Request0Step0Run")));
            assert_eq!(at(&placed, "Request0Step0Result").text, alloc::format!("Not run: {why}."));
            // Run cannot start it either.
            p.update(Msg::Run(0, 0));
            assert!(matches!(p.take_due(), Some(Due::Ask { .. })));
            assert_eq!(p.take_due(), None);
        }
        // No cmd.exe.
        let mut p = on_explorer(FOLDER);
        p.tools = Tools::default();
        p.update(Msg::Draft("x".into()));
        p.update(Msg::Send);
        asking(&mut p);
        p.update(Msg::Answer(0, calls(&[("c1", "run", TAR)])));
        assert_eq!(
            p.requests[0].rounds[0].calls[0].step,
            Step::Refused("cmd.exe is not installed".into())
        );
    }

    #[test]
    fn the_loop_stops_after_its_rounds() {
        let mut p = on_explorer(FOLDER);
        p.update(Msg::Draft("loop".into()));
        p.update(Msg::Send);
        for round in 1..=ROUNDS {
            let due = p.take_due();
            assert!(
                matches!(due, Some(Due::Ask { index: 0, round: r, .. }) if r == round),
                "{due:?}"
            );
            p.update(Msg::Answer(0, calls(&[("c", "run", "{}")])));
        }
        assert_eq!(p.take_due(), None);
        assert_eq!(
            p.requests[0].answer,
            Some(Answer::Failed(alloc::format!(
                "stopped after {ROUNDS} rounds without a final answer"
            )))
        );
        assert!(!p.busy());
        // A failed call of the model ends it too.
        p.update(Msg::Draft("again".into()));
        p.update(Msg::Send);
        asking(&mut p);
        p.update(Msg::Answer(1, Err("agent-relay: not recorded".into())));
        assert_eq!(p.requests[1].answer, Some(Answer::Failed("agent-relay: not recorded".into())));
    }
}
