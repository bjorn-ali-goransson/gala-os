//! Tooltips (tooltips_class32): the tools (window and id, a text) are
//! kept, counted, updated and read back; the tip itself is not shown on
//! hover yet.

use crate::common::*;
use alloc::vec::Vec;

const TTM_ACTIVATE: u32 = WM_USER + 1;
const TTM_SETDELAYTIME: u32 = WM_USER + 3;
const TTM_RELAYEVENT: u32 = WM_USER + 7;
const TTM_GETTOOLCOUNT: u32 = WM_USER + 13;
const TTM_SETTIPBKCOLOR: u32 = WM_USER + 19;
const TTM_SETTIPTEXTCOLOR: u32 = WM_USER + 20;
const TTM_SETMAXTIPWIDTH: u32 = WM_USER + 24;
const TTM_GETMAXTIPWIDTH: u32 = WM_USER + 25;
const TTM_POP: u32 = WM_USER + 28;
const TTM_SETTITLEW: u32 = WM_USER + 33;
const TTM_ADDTOOLW: u32 = WM_USER + 50;
const TTM_DELTOOLW: u32 = WM_USER + 51;
const TTM_NEWTOOLRECTW: u32 = WM_USER + 52;
const TTM_GETTOOLINFOW: u32 = WM_USER + 53;
const TTM_SETTOOLINFOW: u32 = WM_USER + 54;
const TTM_GETTEXTW: u32 = WM_USER + 56;
const TTM_UPDATETIPTEXTW: u32 = WM_USER + 57;
const TTM_ADDTOOLA: u32 = WM_USER + 4;
const TTM_DELTOOLA: u32 = WM_USER + 5;
const TTM_SETWINDOWTHEME: u32 = 0x200b;
/// LPSTR_TEXTCALLBACKW: the text comes from TTN_GETDISPINFO.
const TEXT_CALLBACK: usize = usize::MAX;

struct Tool {
    hwnd: usize,
    id: usize,
    rect: [i32; 4],
    text: Vec<u16>,
}

struct Tips {
    tools: Vec<Tool>,
    max_width: i32,
}

static TIPS: Store<Tips> = Store::new();

fn with<R>(hwnd: Hwnd, f: impl FnOnce(&mut Tips) -> R) -> Option<R> {
    TIPS.with(hwnd, || Tips { tools: Vec::new(), max_width: -1 }, f)
}

/// TOOLINFOW's members: cbSize and uFlags (u32 at 0 and 4), hwnd (8),
/// uId (16), rect (24), hinst (40), lpszText (48).
fn tool_of(info: isize) -> Option<(usize, usize, [i32; 4], usize)> {
    if info == 0 {
        return None;
    }
    let p = info as *const u8;
    // SAFETY: the caller's TOOLINFOW (at least its first 56 bytes).
    unsafe {
        Some((
            p.add(8).cast::<usize>().read_unaligned(),
            p.add(16).cast::<usize>().read_unaligned(),
            p.add(24).cast::<[i32; 4]>().read_unaligned(),
            p.add(48).cast::<usize>().read_unaligned(),
        ))
    }
}

fn text_of(p: usize) -> Vec<u16> {
    if p == TEXT_CALLBACK {
        return Vec::new();
    }
    owned(wide(p as *const u16, 1024))
}

pub extern "system" fn proc_(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        TTM_ADDTOOLW => {
            let Some((owner, id, rect, text)) = tool_of(lparam) else { return 0 };
            let text = text_of(text);
            with(hwnd, |t| {
                if t.tools.try_reserve(1).is_err() {
                    return 0;
                }
                t.tools.retain(|x| !(x.hwnd == owner && x.id == id));
                t.tools.push(Tool { hwnd: owner, id, rect, text });
                1
            })
            .unwrap_or(0)
        }
        TTM_ADDTOOLA => {
            let Some((owner, id, rect, _)) = tool_of(lparam) else { return 0 };
            with(hwnd, |t| {
                if t.tools.try_reserve(1).is_err() {
                    return 0;
                }
                t.tools.push(Tool { hwnd: owner, id, rect, text: Vec::new() });
                1
            })
            .unwrap_or(0)
        }
        TTM_DELTOOLW | TTM_DELTOOLA => {
            if let Some((owner, id, _, _)) = tool_of(lparam) {
                with(hwnd, |t| t.tools.retain(|x| !(x.hwnd == owner && x.id == id)));
            }
            0
        }
        TTM_GETTOOLCOUNT => with(hwnd, |t| t.tools.len() as isize).unwrap_or(0),
        TTM_UPDATETIPTEXTW | TTM_SETTOOLINFOW => {
            if let Some((owner, id, rect, text)) = tool_of(lparam) {
                let text = text_of(text);
                with(hwnd, |t| {
                    if let Some(x) = t.tools.iter_mut().find(|x| x.hwnd == owner && x.id == id) {
                        x.text = text;
                        if msg == TTM_SETTOOLINFOW {
                            x.rect = rect;
                        }
                    }
                });
            }
            0
        }
        TTM_NEWTOOLRECTW => {
            if let Some((owner, id, rect, _)) = tool_of(lparam) {
                with(hwnd, |t| {
                    if let Some(x) = t.tools.iter_mut().find(|x| x.hwnd == owner && x.id == id) {
                        x.rect = rect;
                    }
                });
            }
            0
        }
        TTM_GETTOOLINFOW | TTM_GETTEXTW => {
            let Some((owner, id, _, out)) = tool_of(lparam) else { return 0 };
            let found = with(hwnd, |t| {
                t.tools
                    .iter()
                    .find(|x| x.hwnd == owner && x.id == id)
                    .map(|x| (x.text.clone(), x.rect))
            })
            .flatten();
            match found {
                Some((text, rect)) => {
                    if msg == TTM_GETTOOLINFOW {
                        // SAFETY: the caller's TOOLINFOW.
                        unsafe {
                            (lparam as *mut u8).add(24).cast::<[i32; 4]>().write_unaligned(rect)
                        };
                    }
                    if out != 0 && out != TEXT_CALLBACK {
                        let n = if msg == TTM_GETTEXTW && wparam > 0 {
                            text.len().min(wparam - 1)
                        } else {
                            text.len()
                        };
                        // SAFETY: the caller's text buffer (wparam
                        // characters for TTM_GETTEXTW; large enough
                        // otherwise, as the message requires).
                        unsafe {
                            core::ptr::copy_nonoverlapping(text.as_ptr(), out as *mut u16, n);
                            *(out as *mut u16).add(n) = 0;
                        }
                    }
                    1
                }
                None => 0,
            }
        }
        TTM_SETMAXTIPWIDTH => {
            with(hwnd, |t| core::mem::replace(&mut t.max_width, lparam as i32) as isize)
                .unwrap_or(-1)
        }
        TTM_GETMAXTIPWIDTH => with(hwnd, |t| t.max_width as isize).unwrap_or(-1),
        TTM_ACTIVATE | TTM_SETDELAYTIME | TTM_RELAYEVENT | TTM_POP | TTM_SETTIPBKCOLOR
        | TTM_SETTIPTEXTCOLOR | TTM_SETWINDOWTHEME => 0,
        TTM_SETTITLEW => 1,
        WM_DESTROY => {
            TIPS.remove(hwnd);
            // SAFETY: plain call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // SAFETY: plain call.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
