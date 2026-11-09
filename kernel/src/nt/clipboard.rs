//! The clipboard of a window station (ROADMAP LD step 3, ARCHITECTURE
//! "The clipboard"): one per station, as Windows has it ("About the
//! Clipboard": "the system keeps a serial number for the clipboard for
//! each window station"), shared by every process connected to it.
//! user32's OpenClipboard, EmptyClipboard, SetClipboardData,
//! GetClipboardData and friends are gala's clipboard services
//! (`syscalls/clipboard_calls.rs`) over this state.
//!
//! - **Open**: one thread at a time has the clipboard open, with the
//!   window it named (0 allowed). Another thread, or the same thread with
//!   another window, is refused (ERROR_ACCESS_DENIED in user32).
//! - **Owner**: EmptyClipboard makes the open window (and its process) the
//!   owner (GetClipboardOwner).
//! - **Formats**, in the order they were set: memory formats hold a copy
//!   of the HGLOBAL's bytes, charged to the paged pool quota of the
//!   process that set them (`quota::Held`); GDI and private formats hold
//!   the handle value, meaningful in the setting process only; delayed
//!   formats (SetClipboardData with NULL) hold nothing. Each carries its
//!   serial (the sequence number its setting made), so user32 knows
//!   whether a handle it made for it is still current.
//! - **Close**: if the contents changed while open, a text format without
//!   CF_LOCALE gets CF_LOCALE (the closer's input language), then the
//!   synthesized text formats Windows documents ("Synthesized Clipboard
//!   Formats": CF_TEXT, CF_OEMTEXT and CF_UNICODETEXT from any of them)
//!   are listed after the real ones, and every format listener
//!   (AddClipboardFormatListener) is to get WM_CLIPBOARDUPDATE.
//! - **Sequence number**: grows with every EmptyClipboard and
//!   SetClipboardData.
//!
//! The caller (the services) posts the messages and emits the events;
//! this module only keeps the state, so its rules are host-tested.
//!
//! Seams for the Wayland selection (ROADMAP LD step 3, parts 2 and 3):
//! [`Clipboard::offer`] puts bytes on the clipboard as a kernel-side
//! owner would (empty, set, close in one step, listeners told), and
//! [`Clipboard::text`] reads the current text as UTF-16 bytes; the
//! bridge is to call them, not user32.

use alloc::vec::Vec;

use super::status::NtStatus;
use crate::mm::heap::{KArc, TryVec};
use crate::quota::{Block, Held, Pool};
use crate::sync::IrqMutex;

pub const CF_TEXT: u32 = 1;
pub const CF_BITMAP: u32 = 2;
pub const CF_OEMTEXT: u32 = 7;
pub const CF_DIB: u32 = 8;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_LOCALE: u32 = 16;

/// How a format's data is held (GalaClipboardSet's and Get's `kind`).
/// The bytes of an HGLOBAL, copied into the kernel.
pub const KIND_MEMORY: u32 = 0;
/// A handle value (a GDI object, a private format's), valid in the
/// setting process only.
pub const KIND_HANDLE: u32 = 1;
/// Delayed rendering (SetClipboardData with NULL): no data yet.
pub const KIND_DELAYED: u32 = 2;
/// Synthesized from another text format at close (no data of its own).
pub const KIND_SYNTHESIZED: u32 = 3;

/// The most formats a clipboard holds at once.
pub const MAX_FORMATS: usize = 256;
/// The most format listeners a window station has.
pub const MAX_LISTENERS: usize = 64;
/// What one format costs beyond its bytes (its entry), charged with them.
const ENTRY_COST: u64 = core::mem::size_of::<Format>() as u64;

/// One format on the clipboard.
pub struct Format {
    pub format: u32,
    pub kind: u32,
    /// The bytes (KIND_MEMORY), or the handle value (KIND_HANDLE, 8 bytes).
    pub data: Option<KArc<Vec<u8>>>,
    /// The process that set it.
    pub pid: u32,
    /// The sequence number its setting made.
    pub serial: u32,
    /// KIND_SYNTHESIZED: the format it is made from.
    pub from: u32,
    /// Its bytes and entry, charged to the setter's paged pool quota.
    _charge: Option<Held>,
}

/// Who has the clipboard open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opener {
    pub pid: u32,
    pub tid: u32,
    pub hwnd: u64,
}

/// What GalaClipboardGet hands out for a format.
pub struct Got {
    /// The format whose data this is (the one asked for, or a synthesized
    /// format's source).
    pub source: u32,
    pub kind: u32,
    pub serial: u32,
    pub pid: u32,
    pub data: Option<KArc<Vec<u8>>>,
}

/// GalaClipboardInfo's answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub sequence: u32,
    /// The sequence number of the last EmptyClipboard: data with an older
    /// serial is gone.
    pub emptied: u32,
    /// The window it is open with (0: closed, or open with none).
    pub open_hwnd: u64,
    /// Whether the asking thread has it open.
    pub open_by_caller: bool,
    /// Whether anyone has it open.
    pub open: bool,
    pub owner_hwnd: u64,
    pub count: u32,
}

/// The listeners to tell of a change (WM_CLIPBOARDUPDATE): (pid, hwnd).
pub struct Notify {
    pub listeners: [(u32, u64); MAX_LISTENERS],
    pub count: usize,
}

impl Notify {
    const NONE: Notify = Notify { listeners: [(0, 0); MAX_LISTENERS], count: 0 };

    pub fn iter(&self) -> impl Iterator<Item = &(u32, u64)> {
        self.listeners[..self.count].iter()
    }
}

/// What a close did: whether the contents changed while open, and whom to
/// tell.
pub struct Closed {
    pub changed: bool,
    pub notify: Notify,
}

struct State {
    open: Option<Opener>,
    /// The sequence number when it was opened.
    open_sequence: u32,
    /// The owner: (pid, hwnd), from the last EmptyClipboard.
    owner: Option<(u32, u64)>,
    formats: Vec<Format>,
    sequence: u32,
    emptied: u32,
    listeners: Vec<(u32, u64)>,
}

/// A window station's clipboard (see the module comment).
pub struct Clipboard(IrqMutex<State>);

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    pub const fn new() -> Clipboard {
        Clipboard(IrqMutex::named(
            "winsta.clipboard",
            State {
                open: None,
                open_sequence: 0,
                owner: None,
                formats: Vec::new(),
                sequence: 0,
                emptied: 0,
                listeners: Vec::new(),
            },
        ))
    }

    /// OpenClipboard(`hwnd`) by thread `tid` of `pid`: STATUS_ACCESS_DENIED
    /// while another thread has it open, or this one with another window
    /// (an open with the same window again succeeds). The opener on
    /// refusal, for the event.
    pub fn open(&self, pid: u32, tid: u32, hwnd: u64) -> Result<(), Opener> {
        let mut s = self.0.lock();
        match s.open {
            Some(o) if o.tid != tid || o.pid != pid || o.hwnd != hwnd => Err(o),
            Some(_) => Ok(()),
            None => {
                s.open = Some(Opener { pid, tid, hwnd });
                s.open_sequence = s.sequence;
                Ok(())
            }
        }
    }

    fn require_open(s: &State, pid: u32, tid: u32) -> Result<Opener, NtStatus> {
        match s.open {
            Some(o) if o.pid == pid && o.tid == tid => Ok(o),
            _ => Err(NtStatus::INVALID_LOCK_SEQUENCE),
        }
    }

    /// CloseClipboard by thread `tid` of `pid`: STATUS_INVALID_LOCK_SEQUENCE
    /// unless it has the clipboard open. If the contents changed, CF_LOCALE
    /// (`locale`) and the synthesized text formats are added and the
    /// listeners are returned to be told.
    pub fn close(&self, pid: u32, tid: u32, locale: u32) -> Result<Closed, NtStatus> {
        let mut s = self.0.lock();
        Self::require_open(&s, pid, tid)?;
        Ok(Self::close_locked(&mut s, pid, locale))
    }

    fn close_locked(s: &mut State, pid: u32, locale: u32) -> Closed {
        s.open = None;
        let changed = s.sequence != s.open_sequence;
        let mut notify = Notify::NONE;
        if !changed {
            return Closed { changed, notify };
        }
        Self::synthesize(s, pid, locale);
        for &l in s.listeners.iter().take(MAX_LISTENERS) {
            notify.listeners[notify.count] = l;
            notify.count += 1;
        }
        Closed { changed, notify }
    }

    /// At a close after a change: CF_LOCALE for text without one, then the
    /// text formats missing, synthesized from the best one present
    /// (CF_UNICODETEXT, else CF_TEXT, else CF_OEMTEXT), in Windows' order
    /// (CF_TEXT, CF_OEMTEXT, CF_UNICODETEXT). Nothing is added when memory
    /// runs short (the formats set stay; synthesized ones are missing).
    fn synthesize(s: &mut State, pid: u32, locale: u32) {
        let real = |s: &State, f: u32| {
            s.formats.iter().any(|x| x.format == f && x.kind != KIND_SYNTHESIZED)
        };
        let Some(source) = [CF_UNICODETEXT, CF_TEXT, CF_OEMTEXT].into_iter().find(|&f| real(s, f))
        else {
            return;
        };
        if !s.formats.iter().any(|x| x.format == CF_LOCALE) {
            let mut bytes = Vec::new();
            if bytes.try_extend_from_slice(&locale.to_le_bytes()).is_ok()
                && let Ok(data) = KArc::try_new(bytes)
            {
                let serial = s.sequence;
                let locale = Format {
                    format: CF_LOCALE,
                    kind: KIND_MEMORY,
                    data: Some(data),
                    pid,
                    serial,
                    from: 0,
                    _charge: None,
                };
                if s.formats.try_push(locale).is_err() {
                    return;
                }
            }
        }
        for f in [CF_TEXT, CF_OEMTEXT, CF_UNICODETEXT] {
            if let Some(x) = s.formats.iter_mut().find(|x| x.format == f) {
                if x.kind == KIND_SYNTHESIZED {
                    x.from = source;
                }
                continue;
            }
            let synthesized = Format {
                format: f,
                kind: KIND_SYNTHESIZED,
                data: None,
                pid,
                serial: 0,
                from: source,
                _charge: None,
            };
            if s.formats.try_push(synthesized).is_err() {
                return;
            }
        }
    }

    /// EmptyClipboard: every format goes, the open window (and `pid`)
    /// becomes the owner, the sequence number grows. The previous owner,
    /// if another (WM_DESTROYCLIPBOARD's addressee).
    pub fn empty(&self, pid: u32, tid: u32) -> Result<Option<(u32, u64)>, NtStatus> {
        let mut s = self.0.lock();
        let o = Self::require_open(&s, pid, tid)?;
        let previous = s.owner.filter(|&w| w != (pid, o.hwnd));
        let gone = core::mem::take(&mut s.formats);
        s.owner = Some((pid, o.hwnd));
        s.sequence = s.sequence.wrapping_add(1);
        s.emptied = s.sequence;
        drop(s);
        // The bytes and their charges go outside the lock.
        drop(gone);
        Ok(previous)
    }

    /// SetClipboardData(`format`) by thread `tid` of `pid`: replaces the
    /// format's data or adds it last; `data` was copied (and `charge`
    /// taken) by the caller outside the lock. The new serial.
    pub fn set(
        &self,
        pid: u32,
        tid: u32,
        format: u32,
        kind: u32,
        data: Option<KArc<Vec<u8>>>,
        charge: Option<Held>,
    ) -> Result<u32, NtStatus> {
        let mut s = self.0.lock();
        Self::require_open(&s, pid, tid)?;
        let at = s.formats.iter().position(|x| x.format == format);
        if at.is_none() && s.formats.len() >= MAX_FORMATS {
            return Err(NtStatus::INSUFFICIENT_RESOURCES);
        }
        if at.is_none() {
            // Reserved before anything changes, so the push cannot fail.
            s.formats.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        s.sequence = s.sequence.wrapping_add(1);
        let serial = s.sequence;
        let entry = Format { format, kind, data, pid, serial, from: 0, _charge: charge };
        let old = match at {
            Some(i) => Some(core::mem::replace(&mut s.formats[i], entry)),
            None => {
                // The slot is reserved above.
                s.formats.try_push(entry).map_err(|_| NtStatus::NO_MEMORY)?;
                None
            }
        };
        drop(s);
        drop(old);
        Ok(serial)
    }

    /// GetClipboardData(`format`) by thread `tid` of `pid` (it must have
    /// the clipboard open): the data, or a synthesized format's source's;
    /// STATUS_NOT_FOUND when the format is not there.
    pub fn get(&self, pid: u32, tid: u32, format: u32) -> Result<Got, NtStatus> {
        let s = self.0.lock();
        Self::require_open(&s, pid, tid)?;
        let f = s.formats.iter().find(|x| x.format == format).ok_or(NtStatus::NOT_FOUND)?;
        let f = if f.kind == KIND_SYNTHESIZED {
            s.formats
                .iter()
                .find(|x| x.format == f.from && x.kind != KIND_SYNTHESIZED)
                .ok_or(NtStatus::NOT_FOUND)?
        } else {
            f
        };
        Ok(Got {
            source: f.format,
            kind: f.kind,
            serial: f.serial,
            pid: f.pid,
            data: f.data.clone(),
        })
    }

    /// The formats, in enumeration order, into `out`; how many there are.
    pub fn formats(&self, out: &mut [u32]) -> usize {
        let s = self.0.lock();
        for (o, f) in out.iter_mut().zip(&s.formats) {
            *o = f.format;
        }
        s.formats.len()
    }

    /// The state GalaClipboardInfo reports to thread `tid` of `pid`.
    pub fn info(&self, pid: u32, tid: u32) -> Info {
        let s = self.0.lock();
        Info {
            sequence: s.sequence,
            emptied: s.emptied,
            open_hwnd: s.open.map_or(0, |o| o.hwnd),
            open_by_caller: s.open.is_some_and(|o| o.pid == pid && o.tid == tid),
            open: s.open.is_some(),
            owner_hwnd: s.owner.map_or(0, |o| o.1),
            count: s.formats.len() as u32,
        }
    }

    /// AddClipboardFormatListener: FALSE (Ok(false)) if `hwnd` listens
    /// already; STATUS_INSUFFICIENT_RESOURCES at MAX_LISTENERS.
    pub fn add_listener(&self, pid: u32, hwnd: u64) -> Result<bool, NtStatus> {
        let mut s = self.0.lock();
        if s.listeners.iter().any(|l| l.1 == hwnd) {
            return Ok(false);
        }
        if s.listeners.len() >= MAX_LISTENERS {
            return Err(NtStatus::INSUFFICIENT_RESOURCES);
        }
        s.listeners.try_push((pid, hwnd)).map_err(|_| NtStatus::NO_MEMORY)?;
        Ok(true)
    }

    /// RemoveClipboardFormatListener: whether `hwnd` was listening.
    pub fn remove_listener(&self, hwnd: u64) -> bool {
        let mut s = self.0.lock();
        let before = s.listeners.len();
        s.listeners.retain(|l| l.1 != hwnd);
        s.listeners.len() != before
    }

    /// Thread `tid` of `pid` ended: if it had the clipboard open, it is
    /// closed as CloseClipboard would (the listeners to tell, if it
    /// changed).
    pub fn thread_ended(&self, pid: u32, tid: u32, locale: u32) -> Option<Closed> {
        let mut s = self.0.lock();
        Self::require_open(&s, pid, tid).ok()?;
        Some(Self::close_locked(&mut s, pid, locale))
    }

    /// Process `pid` ended: its listeners go, it no longer owns the
    /// clipboard, and what only it could render or read goes (delayed
    /// formats, handle values); the bytes it copied in stay.
    pub fn process_ended(&self, pid: u32) {
        let mut s = self.0.lock();
        s.listeners.retain(|l| l.0 != pid);
        if s.owner.is_some_and(|o| o.0 == pid) {
            s.owner = None;
        }
        if s.open.is_some_and(|o| o.pid == pid) {
            s.open = None;
        }
        // What goes is freed under the lock (memory only: no handle is
        // closed, no message sent).
        s.formats.retain(|f| f.pid != pid || !matches!(f.kind, KIND_DELAYED | KIND_HANDLE));
    }

    /// Kernel-side owner (the Wayland bridge, ROADMAP LD step 3 part 3):
    /// replaces the contents with `format`'s `bytes` in one step, as
    /// OpenClipboard, EmptyClipboard, SetClipboardData and CloseClipboard
    /// would for a process `pid` without a window; refused
    /// (STATUS_ACCESS_DENIED) while a program has the clipboard open. The
    /// listeners to tell.
    pub fn offer(
        &self,
        pid: u32,
        format: u32,
        bytes: KArc<Vec<u8>>,
        charge: Option<Held>,
        locale: u32,
    ) -> Result<Notify, NtStatus> {
        // The new list is made outside the lock.
        let mut fresh = Vec::new();
        fresh.try_reserve(4).map_err(|_| NtStatus::NO_MEMORY)?;
        let mut s = self.0.lock();
        if s.open.is_some() {
            return Err(NtStatus::ACCESS_DENIED);
        }
        // Emptied, then set: two steps of the sequence number, as a
        // program's EmptyClipboard and SetClipboardData make.
        s.open_sequence = s.sequence;
        s.emptied = s.sequence.wrapping_add(1);
        s.sequence = s.sequence.wrapping_add(2);
        let serial = s.sequence;
        let entry = Format {
            format,
            kind: KIND_MEMORY,
            data: Some(bytes),
            pid,
            serial,
            from: 0,
            _charge: charge,
        };
        fresh.try_push(entry).map_err(|_| NtStatus::NO_MEMORY)?;
        let gone = core::mem::replace(&mut s.formats, fresh);
        s.owner = Some((pid, 0));
        s.open = Some(Opener { pid, tid: 0, hwnd: 0 });
        let closed = Self::close_locked(&mut s, pid, locale);
        drop(s);
        drop(gone);
        Ok(closed.notify)
    }

    /// The text on the clipboard as UTF-16 bytes (CF_UNICODETEXT's own,
    /// else None: the 8-bit formats' conversion is user32's), for a
    /// kernel-side reader (the Wayland bridge).
    pub fn text(&self) -> Option<KArc<Vec<u8>>> {
        let s = self.0.lock();
        s.formats
            .iter()
            .find(|x| x.format == CF_UNICODETEXT && x.kind == KIND_MEMORY)
            .and_then(|x| x.data.clone())
    }

    /// One line for the state dump.
    pub fn summary(&self) -> Summary {
        let s = self.0.lock();
        let mut formats = [0u32; 8];
        for (o, f) in formats.iter_mut().zip(&s.formats) {
            *o = f.format;
        }
        let bytes = s.formats.iter().map(|f| f.data.as_ref().map_or(0, |d| d.len())).sum();
        Summary {
            sequence: s.sequence,
            open: s.open,
            owner: s.owner,
            count: s.formats.len(),
            formats,
            bytes,
            listeners: s.listeners.len(),
        }
    }
}

/// A clipboard's state for the state dump.
pub struct Summary {
    pub sequence: u32,
    pub open: Option<Opener>,
    pub owner: Option<(u32, u64)>,
    pub count: usize,
    /// The first eight formats.
    pub formats: [u32; 8],
    pub bytes: usize,
    pub listeners: usize,
}

/// WM_CLIPBOARDUPDATE.
pub const WM_CLIPBOARDUPDATE: u32 = 0x031d;
/// WM_DESTROYCLIPBOARD (to the owner EmptyClipboard replaces).
pub const WM_DESTROYCLIPBOARD: u32 = 0x0307;
/// The input language gala's keyboard has (US English, as user32's
/// GetKeyboardLayout says): CF_LOCALE when a thread ends with the
/// clipboard open and changed.
pub const GALA_INPUT_LOCALE: u32 = 0x0409;

/// Posts WM_CLIPBOARDUPDATE to the listeners `notify` names through
/// `windows` (their session's registry); a listener whose window is gone
/// is taken off `clipboard`'s list. How many were told.
pub fn tell(windows: &super::window::Windows, clipboard: &Clipboard, notify: &Notify) -> usize {
    let mut told = 0;
    for &(_, hwnd) in notify.iter() {
        match windows.post_from_system(hwnd, WM_CLIPBOARDUPDATE, 0, 0) {
            Ok(event) => {
                told += 1;
                if let Some(e) = event {
                    super::wait::set_signal(&e, 1);
                }
            }
            Err(NtStatus::INVALID_HANDLE) => {
                clipboard.remove_listener(hwnd);
            }
            Err(_) => {}
        }
    }
    told
}

/// The window station process `process` is connected to, if it connected
/// (its first window station call, or a clipboard call, resolved it).
fn station_of(process: &super::process::Process) -> Option<super::object::ObjectRef> {
    match process.ui.resolved() {
        Some(Ok((station, _))) => Some(station),
        _ => None,
    }
}

/// Thread `tid` of `process` ended: the clipboard it had open is closed
/// (nt::thread's end of a thread).
pub fn thread_ended(process: &super::process::Process, tid: u64) {
    let Some(station) = station_of(process) else { return };
    let Some(s) = super::winsta::station_of(&station) else { return };
    if let Some(closed) = s.clipboard.thread_ended(process.id as u32, tid as u32, GALA_INPUT_LOCALE)
    {
        tell(&process.desk.windows, &s.clipboard, &closed.notify);
    }
}

/// Process `process` ended: what only it held goes (`Clipboard::process_ended`).
pub fn process_ended(process: &super::process::Process) {
    let Some(station) = station_of(process) else { return };
    if let Some(s) = super::winsta::station_of(&station) {
        s.clipboard.process_ended(process.id as u32);
    }
}

/// Charges `bytes` of data (and its entry) to `block`'s paged pool:
/// STATUS_QUOTA_EXCEEDED over its limit.
pub fn charge(block: &KArc<Block>, bytes: usize) -> Result<Held, NtStatus> {
    Held::try_new(block, Pool::Paged, bytes as u64 + ENTRY_COST)
        .map_err(|_| NtStatus::QUOTA_EXCEEDED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(b: &[u8]) -> Option<KArc<Vec<u8>>> {
        let mut v = Vec::new();
        v.try_extend_from_slice(b).unwrap();
        Some(KArc::try_new(v).unwrap())
    }

    fn list(c: &Clipboard) -> Vec<u32> {
        let mut out = [0u32; 16];
        let n = c.formats(&mut out);
        let mut v = Vec::new();
        v.try_extend_from_slice(&out[..n]).unwrap();
        v
    }

    #[test]
    fn one_opener_at_a_time() {
        let c = Clipboard::new();
        assert!(c.open(4, 10, 0x100).is_ok());
        // The same thread and window again; another window or thread not.
        assert!(c.open(4, 10, 0x100).is_ok());
        assert_eq!(c.open(4, 10, 0x200), Err(Opener { pid: 4, tid: 10, hwnd: 0x100 }));
        assert!(c.open(8, 20, 0x100).is_err());
        assert!(c.open(8, 20, 0).is_err());
        assert_eq!(c.close(8, 20, 0x409).err().map(|e| e.0), Some(0xc000_001e));
        assert!(!c.close(4, 10, 0x409).ok().unwrap().changed);
        assert!(c.close(4, 10, 0x409).is_err());
        assert!(c.open(8, 20, 0).is_ok());
        assert_eq!(c.info(8, 20).open_hwnd, 0);
        assert!(c.info(8, 20).open_by_caller && !c.info(4, 10).open_by_caller);
    }

    #[test]
    fn empty_set_get_and_synthesized_text() {
        let c = Clipboard::new();
        assert!(c.empty(4, 10).is_err());
        assert!(c.add_listener(4, 0x100).unwrap());
        assert!(!c.add_listener(4, 0x100).unwrap());
        c.open(4, 10, 0x100).unwrap();
        assert_eq!(c.empty(4, 10).unwrap(), None);
        let emptied = c.info(4, 10).sequence;
        let serial = c.set(4, 10, CF_UNICODETEXT, KIND_MEMORY, bytes(b"h\0i\0\0\0"), None).unwrap();
        assert!(serial > emptied);
        // Not synthesized while open.
        assert_eq!(list(&c), [CF_UNICODETEXT]);
        let closed = c.close(4, 10, 0x409).ok().unwrap();
        assert!(closed.changed);
        assert_eq!(closed.notify.count, 1);
        assert_eq!(closed.notify.listeners[0], (4, 0x100));
        assert_eq!(list(&c), [CF_UNICODETEXT, CF_LOCALE, CF_TEXT, CF_OEMTEXT]);
        let i = c.info(8, 20);
        assert_eq!((i.owner_hwnd, i.count, i.emptied), (0x100, 4, emptied));
        // Another process reads them: CF_TEXT comes from CF_UNICODETEXT.
        assert!(c.get(8, 20, CF_TEXT).is_err());
        c.open(8, 20, 0x300).unwrap();
        let got = c.get(8, 20, CF_TEXT).ok().unwrap();
        assert_eq!((got.source, got.serial, got.pid), (CF_UNICODETEXT, serial, 4));
        let got = c.get(8, 20, CF_LOCALE).ok().unwrap();
        assert_eq!(got.data.unwrap().as_slice(), 0x409u32.to_le_bytes());
        assert_eq!(c.get(8, 20, CF_DIB).err().map(|e| e.0), Some(NtStatus::NOT_FOUND.0));
        // It replaces them with CF_TEXT and becomes the owner.
        assert_eq!(c.empty(8, 20).unwrap(), Some((4, 0x100)));
        c.set(8, 20, CF_TEXT, KIND_MEMORY, bytes(b"x\0"), None).unwrap();
        c.close(8, 20, 0x409).ok().unwrap();
        assert_eq!(list(&c), [CF_TEXT, CF_LOCALE, CF_OEMTEXT, CF_UNICODETEXT]);
        c.open(4, 10, 0x100).unwrap();
        assert_eq!(c.get(4, 10, CF_UNICODETEXT).ok().unwrap().source, CF_TEXT);
        // A close without a change tells nobody.
        assert_eq!(c.close(4, 10, 0x409).ok().unwrap().notify.count, 0);
        assert!(c.remove_listener(0x100));
        assert!(!c.remove_listener(0x100));
    }

    #[test]
    fn a_real_format_replaces_a_synthesized_one() {
        let c = Clipboard::new();
        c.open(4, 10, 0).unwrap();
        c.empty(4, 10).unwrap();
        c.set(4, 10, CF_TEXT, KIND_MEMORY, bytes(b"a\0"), None).unwrap();
        c.close(4, 10, 0x409).ok().unwrap();
        c.open(4, 10, 0).unwrap();
        c.set(4, 10, CF_UNICODETEXT, KIND_MEMORY, bytes(b"b\0\0\0"), None).unwrap();
        c.close(4, 10, 0x409).ok().unwrap();
        assert_eq!(list(&c), [CF_TEXT, CF_LOCALE, CF_OEMTEXT, CF_UNICODETEXT]);
        c.open(4, 10, 0).unwrap();
        assert_eq!(c.get(4, 10, CF_UNICODETEXT).ok().unwrap().kind, KIND_MEMORY);
        // OEM text now comes from CF_UNICODETEXT, the best source.
        assert_eq!(c.get(4, 10, CF_OEMTEXT).ok().unwrap().source, CF_UNICODETEXT);
    }

    #[test]
    fn ends_release_what_they_held() {
        let c = Clipboard::new();
        c.add_listener(4, 0x100).unwrap();
        c.open(4, 10, 0x100).unwrap();
        c.empty(4, 10).unwrap();
        c.set(4, 10, CF_BITMAP, KIND_HANDLE, bytes(&0x1234u64.to_le_bytes()), None).unwrap();
        c.set(4, 10, CF_DIB, KIND_DELAYED, None, None).unwrap();
        c.set(4, 10, CF_TEXT, KIND_MEMORY, bytes(b"t\0"), None).unwrap();
        // The thread's end closes it as CloseClipboard would.
        assert!(c.thread_ended(4, 11, 0x409).is_none());
        let closed = c.thread_ended(4, 10, 0x409).unwrap();
        assert_eq!(closed.notify.count, 1);
        c.process_ended(4);
        let i = c.info(8, 20);
        assert_eq!((i.owner_hwnd, i.open), (0, false));
        assert_eq!(list(&c), [CF_TEXT, CF_LOCALE, CF_OEMTEXT, CF_UNICODETEXT]);
        assert!(!c.remove_listener(0x100));
    }

    #[test]
    fn charges_go_back_with_the_data() {
        let block = Block::with_limits(4, "x", 4096, 4096, 16).ok().unwrap();
        let c = Clipboard::new();
        c.open(4, 10, 0).unwrap();
        let held = charge(&block, 1000).ok().unwrap();
        c.set(4, 10, CF_TEXT, KIND_MEMORY, bytes(&[1; 1000]), Some(held)).unwrap();
        assert!(block.pool(Pool::Paged).0 >= 1000);
        assert!(charge(&block, 4000).is_err());
        c.empty(4, 10).unwrap();
        assert_eq!(block.pool(Pool::Paged).0, 0);
    }

    #[test]
    fn a_kernel_offer_waits_for_programs_and_tells_listeners() {
        let c = Clipboard::new();
        c.add_listener(4, 0x100).unwrap();
        c.open(4, 10, 0).unwrap();
        let text = || bytes(b"w\0\0\0").unwrap();
        assert!(c.offer(1, CF_UNICODETEXT, text(), None, 0x409).is_err());
        c.close(4, 10, 0x409).ok().unwrap();
        let before = c.info(4, 10).sequence;
        let notify = c.offer(1, CF_UNICODETEXT, text(), None, 0x409).ok().unwrap();
        assert_eq!(notify.count, 1);
        let i = c.info(4, 10);
        assert!(i.sequence > before && i.emptied > before && !i.open);
        assert_eq!(c.text().unwrap().as_slice(), b"w\0\0\0");
        assert_eq!(list(&c), [CF_UNICODETEXT, CF_LOCALE, CF_TEXT, CF_OEMTEXT]);
    }
}
