//! GalaSession: gala's session service (ROADMAP RS; ARCHITECTURE
//! "Presenters (B3a)"). So far what shows a session's desktop: a query, a
//! presenter attached (None, Console, Test) and a Test frame's pixels read
//! back (screen checks inside the guest, also in gala-um). B4's session
//! manager adds create, logoff and list here. Every operation needs
//! SeTcbPrivilege (another session's pixels are its users').

use alloc::vec::Vec;

use super::super::desktop::{self, Kind, Rect};
use super::super::status::NtStatus;
use super::Call;

/// GalaSession's operations.
pub const SESSION_QUERY: u64 = 0;
pub const SESSION_PRESENT: u64 = 1;
pub const SESSION_FRAME: u64 = 2;

/// SeTcbPrivilege's LUID.
const SE_TCB: u64 = 7;

/// The largest area SESSION_FRAME copies at once (a 4096 x 4096 frame).
const MAX_PIXELS: u64 = 4096 * 4096;

/// GalaSession(Operation, Session, Argument, Buffer, Length):
/// - SESSION_QUERY: Buffer gets five u32 (Length at least 20): what shows
///   the desktop (0 none, 1 console, 2 test), whether it is live, its
///   width and height (0 before it is live), its windows.
/// - SESSION_PRESENT: attaches presenter Argument (0 none, 1 console, 2
///   test); Buffer is a Test frame's size, width | height << 32 (0: the
///   desktop's). Console only for the console session on a machine with a
///   screen (a console switch is not there yet: STATUS_NOT_SUPPORTED).
/// - SESSION_FRAME: Argument points to a 32-byte record, in: x, y, width,
///   height (u32) of an area of the Test frame showing the desktop; out:
///   the frame's width and height (u32) and its flushes (u64). Buffer gets
///   the area's 0x00RRGGBB pixels row after row (Length bytes, at least 4
///   x width x height). STATUS_INVALID_DEVICE_STATE when no Test presenter
///   shows the desktop; STATUS_INVALID_PARAMETER for an area outside it.
///
/// STATUS_PRIVILEGE_NOT_HELD without SeTcbPrivilege, STATUS_NOT_FOUND for
/// a session that does not exist, STATUS_NOT_SUPPORTED where there are no
/// desktops (the server profile, no screen and no `presenter=`).
pub(super) fn gala_session(call: &mut Call<'_>) -> NtStatus {
    let (op, id, arg, buffer, length) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2), arg!(call, 3), arg!(call, 4));
    if !super::security_calls::effective_privilege(call, SE_TCB) {
        return NtStatus::PRIVILEGE_NOT_HELD;
    }
    let Some(desk) = super::super::session::desk(id) else { return NtStatus::NOT_FOUND };
    let shell = &desk.shell;
    match op {
        SESSION_QUERY => {
            if length < 20 {
                return NtStatus::BUFFER_TOO_SMALL;
            }
            let (kind, live, (w, h), windows) = desktop::query(shell);
            let mut out = [0u8; 20];
            for (i, v) in [kind as u32, u32::from(live), w, h, windows as u32].iter().enumerate() {
                out[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
            }
            if call.process.write(buffer, &out).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            NtStatus::SUCCESS
        }
        SESSION_PRESENT => {
            let Some(kind) = Kind::from_number(arg) else { return NtStatus::INVALID_PARAMETER };
            let size = match (buffer as u32, (buffer >> 32) as u32) {
                (0, 0) => None,
                (w, h)
                    if w > 0
                        && h > 0
                        && w <= desktop::MAX_FRAME_SIDE
                        && h <= desktop::MAX_FRAME_SIDE =>
                {
                    Some((w, h))
                }
                _ => return NtStatus::INVALID_PARAMETER,
            };
            if size.is_some() && kind != Kind::Test {
                return NtStatus::INVALID_PARAMETER;
            }
            match desktop::attach(shell, kind, size) {
                Ok(_) => NtStatus::SUCCESS,
                Err(e) => {
                    crate::warn!(
                        "desktop: pid {} could not show session {id} by {}: {e:?}",
                        call.process.id,
                        kind.name()
                    );
                    e
                }
            }
        }
        SESSION_FRAME => {
            let mut record = [0u8; 32];
            if call.process.read(arg, &mut record[..16]).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            let field = |i: usize| {
                u32::from_le_bytes([
                    record[4 * i],
                    record[4 * i + 1],
                    record[4 * i + 2],
                    record[4 * i + 3],
                ])
            };
            let r = Rect {
                x: i64::from(field(0)),
                y: i64::from(field(1)),
                w: i64::from(field(2)),
                h: i64::from(field(3)),
            };
            let n = r.w as u64 * r.h as u64;
            if n == 0 || n > MAX_PIXELS {
                return NtStatus::INVALID_PARAMETER;
            }
            if length < 4 * n {
                return NtStatus::BUFFER_TOO_SMALL;
            }
            let mut pixels: Vec<u32> = Vec::new();
            if pixels.try_reserve_exact(n as usize).is_err() {
                return NtStatus::NO_MEMORY;
            }
            pixels.resize(n as usize, 0);
            let ((w, h), flushes) = tri!(desktop::read_frame(shell, r, &mut pixels));
            let mut bytes: Vec<u8> = Vec::new();
            if bytes.try_reserve_exact(4 * n as usize).is_err() {
                return NtStatus::NO_MEMORY;
            }
            bytes.extend(pixels.iter().flat_map(|p| p.to_le_bytes()));
            if call.process.write(buffer, &bytes).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            record[16..20].copy_from_slice(&w.to_le_bytes());
            record[20..24].copy_from_slice(&h.to_le_bytes());
            record[24..32].copy_from_slice(&flushes.to_le_bytes());
            if call.process.write(arg + 16, &record[16..]).is_err() {
                return NtStatus::ACCESS_VIOLATION;
            }
            NtStatus::SUCCESS
        }
        _ => NtStatus::INVALID_PARAMETER,
    }
}
