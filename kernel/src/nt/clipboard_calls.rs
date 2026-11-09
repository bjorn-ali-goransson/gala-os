//! gala's clipboard services over the caller's window station's clipboard
//! (`nt::clipboard`), for gala's user32 (ARCHITECTURE "The clipboard"):
//! open, close, empty, set, get, the state, the formats and the format
//! listeners. Each names the clipboard by the caller's connection: the
//! first call connects the process to its window station as a window
//! station call would.
//!
//! Events (category `ipc`, `"ev":"clipboard"`): `open`, `open denied`
//! (with the holder), `empty`, `set` and `get` (format, kind, size; a
//! synthesized format's `source`), `close` (whether it changed, the
//! listeners told), `listener`.

use alloc::vec::Vec;

use super::super::clipboard::{
    self, Clipboard, KIND_DELAYED, KIND_HANDLE, KIND_MEMORY, MAX_FORMATS, WM_DESTROYCLIPBOARD,
};
use super::super::object::ObjectRef;
use super::super::status::NtStatus;
use super::super::thread;
use super::super::winsta::station_of;
use super::Call;
use crate::events::{Category, Utf16};
use crate::mm::heap::KArc;

/// GalaClipboardGet's record: size, serial, source format, kind (u32 each).
const GET_INFO: usize = 16;
/// GalaClipboardInfo's record: sequence, flags, the open window, the
/// owner, the format count, the last empty's sequence number.
const INFO_SIZE: usize = 32;
/// GalaClipboardInfo's flags.
const INFO_OPEN_BY_CALLER: u32 = 1;
const INFO_OPEN: u32 = 2;

fn tid() -> u32 {
    thread::current_id().unwrap_or(0) as u32
}

impl Call<'_> {
    /// The caller's window station (its clipboard's holder).
    fn clipboard_station(&self) -> Result<ObjectRef, NtStatus> {
        Ok(self.connection()?.0)
    }

    fn pid(&self) -> u32 {
        self.process.id as u32
    }

    /// One `"ev":"clipboard"` event: `op`, the caller and its station,
    /// then `rest` (JSON members, or empty).
    fn clipboard_event(&self, station: &ObjectRef, op: &str, rest: core::fmt::Arguments) {
        let name = station_of(station).map_or(&[][..], |s| &s.name);
        crate::events::emit(
            Category::Ipc,
            "clipboard",
            format_args!(
                r#""op":"{op}","pid":{},"tid":{},"session":{},"station":{}{rest}"#,
                self.pid(),
                tid(),
                self.process.session,
                Utf16(name)
            ),
        );
    }
}

/// The clipboard of window station `station`.
fn clip_of(station: &ObjectRef) -> Option<&Clipboard> {
    station_of(station).map(|s| &s.clipboard)
}

/// GalaClipboardOpen(Hwnd): OpenClipboard. STATUS_ACCESS_DENIED while
/// another thread has the clipboard open, or this one with another window.
pub(super) fn gala_clipboard_open(call: &mut Call<'_>) -> NtStatus {
    let hwnd = tri!(call.arg(0));
    let station = tri!(call.clipboard_station());
    let Some(clip) = clip_of(&station) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    match clip.open(call.pid(), tid(), hwnd) {
        Ok(()) => {
            call.clipboard_event(&station, "open", format_args!(r#","hwnd":{hwnd}"#));
            NtStatus::SUCCESS
        }
        Err(by) => {
            call.clipboard_event(
                &station,
                "open denied",
                format_args!(
                    r#","hwnd":{hwnd},"holder_pid":{},"holder_tid":{},"holder_hwnd":{}"#,
                    by.pid, by.tid, by.hwnd
                ),
            );
            NtStatus::ACCESS_DENIED
        }
    }
}

/// GalaClipboardClose(Locale): CloseClipboard; `Locale` is the caller's
/// input language, CF_LOCALE if text was set without one. The format
/// listeners get WM_CLIPBOARDUPDATE if the contents changed.
/// STATUS_INVALID_LOCK_SEQUENCE unless the caller has it open.
pub(super) fn gala_clipboard_close(call: &mut Call<'_>) -> NtStatus {
    let locale = tri!(call.arg(0)) as u32;
    let station = tri!(call.clipboard_station());
    let Some(clip) = clip_of(&station) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let closed = tri!(clip.close(call.pid(), tid(), locale));
    let told = clipboard::tell(&call.process.desk.windows, clip, &closed.notify);
    call.clipboard_event(
        &station,
        "close",
        format_args!(r#","changed":{},"listeners":{told}"#, closed.changed),
    );
    NtStatus::SUCCESS
}

/// GalaClipboardEmpty(): EmptyClipboard; the open window becomes the owner.
/// The owner it replaces (another window) gets WM_DESTROYCLIPBOARD,
/// posted (Windows sends it).
pub(super) fn gala_clipboard_empty(call: &mut Call<'_>) -> NtStatus {
    let station = tri!(call.clipboard_station());
    let Some(clip) = clip_of(&station) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let previous = tri!(clip.empty(call.pid(), tid()));
    let (was_pid, was_hwnd) = previous.unwrap_or((0, 0));
    if was_hwnd != 0
        && let Ok(Some(e)) =
            call.process.desk.windows.post_from_system(was_hwnd, WM_DESTROYCLIPBOARD, 0, 0)
    {
        super::super::wait::set_signal(&e, 1);
    }
    call.clipboard_event(
        &station,
        "empty",
        format_args!(r#","previous_owner_pid":{was_pid},"previous_owner_hwnd":{was_hwnd}"#),
    );
    NtStatus::SUCCESS
}

/// GalaClipboardSet(Format, Kind, *Data, Length, *Serial):
/// SetClipboardData. Kind 0: `Length` bytes at `Data` (an HGLOBAL's),
/// copied into the kernel and charged to the caller's paged pool quota
/// (STATUS_QUOTA_EXCEEDED over it); 1: a handle value (8 bytes at `Data`),
/// valid in the caller only; 2: delayed rendering (no data). The new
/// serial goes to `Serial`. STATUS_INVALID_LOCK_SEQUENCE unless the
/// caller has the clipboard open.
pub(super) fn gala_clipboard_set(call: &mut Call<'_>) -> NtStatus {
    let (format, kind) = (tri!(call.arg(0)) as u32, tri!(call.arg(1)) as u32);
    let (ptr, length, out) = (tri!(call.arg(2)), tri!(call.arg(3)) as usize, tri!(call.arg(4)));
    let station = tri!(call.clipboard_station());
    let Some(clip) = clip_of(&station) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    // Checked before anything is copied: a caller without the clipboard
    // open costs nothing.
    if !clip.info(call.pid(), tid()).open_by_caller {
        return NtStatus::INVALID_LOCK_SEQUENCE;
    }
    let (data, charge) = match kind {
        KIND_MEMORY | KIND_HANDLE => {
            if kind == KIND_HANDLE && length != 8 {
                return NtStatus::INVALID_PARAMETER;
            }
            let charge = tri!(clipboard::charge(&call.process.quota, length));
            let mut bytes = Vec::new();
            if bytes.try_reserve_exact(length).is_err() {
                return NtStatus::NO_MEMORY;
            }
            bytes.resize(length, 0);
            if call.process.read(ptr, &mut bytes).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            let Ok(data) = KArc::try_new(bytes) else { return NtStatus::NO_MEMORY };
            (Some(data), Some(charge))
        }
        KIND_DELAYED => {
            super::super::ledger::record(
                "GalaClipboardSet",
                "delayed rendering (no WM_RENDERFORMAT is sent)",
                u64::from(format),
            );
            (None, None)
        }
        _ => return NtStatus::INVALID_PARAMETER,
    };
    let serial = tri!(clip.set(call.pid(), tid(), format, kind, data, charge));
    if out != 0 {
        tri!(call.write_u32(out, serial));
    }
    call.clipboard_event(
        &station,
        "set",
        format_args!(r#","format":{format},"kind":{kind},"size":{length},"serial":{serial}"#),
    );
    NtStatus::SUCCESS
}

/// GalaClipboardGet(Format, *Buffer, Length, *Info): GetClipboardData's
/// data. `Info` gets 16 bytes: the size, the serial, the format the data
/// is (a synthesized format's source: user32 converts) and its kind. The
/// data goes to `Buffer` if `Length` holds it, else
/// STATUS_BUFFER_TOO_SMALL (the record written). STATUS_NOT_FOUND for a
/// format not there, a delayed one, or another process's handle value;
/// STATUS_INVALID_LOCK_SEQUENCE unless the caller has the clipboard open.
pub(super) fn gala_clipboard_get(call: &mut Call<'_>) -> NtStatus {
    let format = tri!(call.arg(0)) as u32;
    let (buffer, length, info) = (tri!(call.arg(1)), tri!(call.arg(2)) as usize, tri!(call.arg(3)));
    let station = tri!(call.clipboard_station());
    let Some(clip) = clip_of(&station) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let got = tri!(clip.get(call.pid(), tid(), format));
    match got.kind {
        KIND_DELAYED => {
            super::super::ledger::record(
                "GalaClipboardGet",
                "delayed rendering (no WM_RENDERFORMAT is sent)",
                u64::from(format),
            );
            return NtStatus::NOT_FOUND;
        }
        KIND_HANDLE if got.pid != call.pid() => {
            super::super::ledger::record(
                "GalaClipboardGet",
                "a GDI or private format from another process",
                u64::from(format),
            );
            return NtStatus::NOT_FOUND;
        }
        _ => {}
    }
    let size = got.data.as_ref().map_or(0, |d| d.len());
    let mut record = [0u8; GET_INFO];
    record[0..4].copy_from_slice(&(size as u32).to_le_bytes());
    record[4..8].copy_from_slice(&got.serial.to_le_bytes());
    record[8..12].copy_from_slice(&got.source.to_le_bytes());
    record[12..16].copy_from_slice(&got.kind.to_le_bytes());
    if call.process.write(info, &record).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    if length < size {
        return NtStatus::BUFFER_TOO_SMALL;
    }
    if let Some(d) = &got.data
        && call.process.write(buffer, d).is_err()
    {
        return NtStatus::ACCESS_VIOLATION;
    }
    call.clipboard_event(
        &station,
        "get",
        format_args!(
            r#","format":{format},"source":{},"kind":{},"size":{size},"from_pid":{}"#,
            got.source, got.kind, got.pid
        ),
    );
    NtStatus::SUCCESS
}

/// GalaClipboardInfo(*Record): 32 bytes: the sequence number, flags (1:
/// the caller has it open, 2: someone has), the open window (u64), the
/// owner (u64), the format count and the sequence number of the last
/// EmptyClipboard.
pub(super) fn gala_clipboard_info(call: &mut Call<'_>) -> NtStatus {
    let out = tri!(call.arg(0));
    let station = tri!(call.clipboard_station());
    let Some(clip) = clip_of(&station) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let i = clip.info(call.pid(), tid());
    let mut r = [0u8; INFO_SIZE];
    let flags =
        if i.open_by_caller { INFO_OPEN_BY_CALLER } else { 0 } | if i.open { INFO_OPEN } else { 0 };
    r[0..4].copy_from_slice(&i.sequence.to_le_bytes());
    r[4..8].copy_from_slice(&flags.to_le_bytes());
    r[8..16].copy_from_slice(&i.open_hwnd.to_le_bytes());
    r[16..24].copy_from_slice(&i.owner_hwnd.to_le_bytes());
    r[24..28].copy_from_slice(&i.count.to_le_bytes());
    r[28..32].copy_from_slice(&i.emptied.to_le_bytes());
    if call.process.write(out, &r).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

/// GalaClipboardFormats(*Formats, Max, *Count): the formats in
/// enumeration order (EnumClipboardFormats; the synthesized ones after the
/// real ones), at most `Max` of them; `Count` gets how many there are.
pub(super) fn gala_clipboard_formats(call: &mut Call<'_>) -> NtStatus {
    let (out, max, count) = (tri!(call.arg(0)), tri!(call.arg(1)) as usize, tri!(call.arg(2)));
    let station = tri!(call.clipboard_station());
    let Some(clip) = clip_of(&station) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let mut list = [0u32; MAX_FORMATS];
    let n = clip.formats(&mut list);
    let mut bytes = [0u8; 4 * MAX_FORMATS];
    let shown = n.min(max).min(MAX_FORMATS);
    for (b, f) in bytes.chunks_exact_mut(4).zip(&list[..shown]) {
        b.copy_from_slice(&f.to_le_bytes());
    }
    if shown > 0 && call.process.write(out, &bytes[..4 * shown]).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    tri!(call.write_u32(count, n as u32));
    NtStatus::SUCCESS
}

/// GalaClipboardListener(Hwnd, Add): AddClipboardFormatListener (Add 1)
/// or RemoveClipboardFormatListener (0). Adding needs a window of the
/// caller's (STATUS_INVALID_HANDLE); removing one not listening is
/// STATUS_NOT_FOUND; STATUS_INSUFFICIENT_RESOURCES at the station's limit
/// (MAX_LISTENERS).
pub(super) fn gala_clipboard_listener(call: &mut Call<'_>) -> NtStatus {
    let (hwnd, add) = (tri!(call.arg(0)), tri!(call.arg(1)) != 0);
    let station = tri!(call.clipboard_station());
    let Some(clip) = clip_of(&station) else { return NtStatus::OBJECT_TYPE_MISMATCH };
    if add {
        if call.process.desk.windows.owner_pid(hwnd) != Some(call.pid()) {
            return NtStatus::INVALID_HANDLE;
        }
        tri!(clip.add_listener(call.pid(), hwnd));
    } else if !clip.remove_listener(hwnd) {
        return NtStatus::NOT_FOUND;
    }
    call.clipboard_event(&station, "listener", format_args!(r#","hwnd":{hwnd},"add":{add}"#));
    NtStatus::SUCCESS
}
