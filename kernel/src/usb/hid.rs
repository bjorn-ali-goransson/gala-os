//! HID reports (Device Class Definition for HID 1.11, HID Usage Tables)
//! turned into key presses and pointer moves: a boot-protocol keyboard's
//! 8-byte report diffed against the one before, and a mouse's report laid
//! out by its report descriptor (buttons, X, Y, wheel), or the boot
//! layout when the descriptor gives none of them.
//!
//! Pure logic, so that it runs on the host (`tests`).

/// What a report meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A key (a usage of the Keyboard/Keypad page; the modifiers are
    /// 0xe0..=0xe7) went down or up.
    Key { usage: u8, down: bool },
    /// The pointer moved (dy screen-down), the buttons now held (1 left,
    /// 2 right, 4 middle), wheel notches (positive away from the user).
    Pointer { dx: i32, dy: i32, buttons: u8, dz: i32 },
}

/// A field of a report: its bit offset (after the report ID byte), its
/// size in bits, and whether it is signed (its logical minimum below 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub offset: u16,
    pub size: u8,
    pub signed: bool,
}

impl Field {
    /// The field's value in `data` (0 where the report is too short).
    pub fn get(&self, data: &[u8]) -> i32 {
        let mut v: u32 = 0;
        for i in 0..usize::from(self.size.min(32)) {
            let bit = usize::from(self.offset) + i;
            let set = data.get(bit / 8).is_some_and(|b| b >> (bit % 8) & 1 != 0);
            v |= u32::from(set) << i;
        }
        let size = u32::from(self.size.clamp(1, 32));
        if self.signed && size < 32 && v >> (size - 1) & 1 != 0 {
            (v | !0u32 << size) as i32
        } else {
            v as i32
        }
    }
}

/// Where a mouse report keeps its buttons, moves and wheel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseLayout {
    /// The report's ID (its first byte) when the descriptor declares IDs.
    pub report_id: Option<u8>,
    /// Buttons 1 (left), 2 (right), 3 (middle).
    pub buttons: [Option<Field>; 3],
    pub x: Option<Field>,
    pub y: Option<Field>,
    pub wheel: Option<Field>,
}

impl MouseLayout {
    /// The boot protocol's report: buttons in byte 0, X and Y signed
    /// bytes 1 and 2 (a wheel in byte 3 where the device sends one).
    pub fn boot() -> Self {
        let bit = |n: u16| Some(Field { offset: n, size: 1, signed: false });
        let byte = |n: u16| Some(Field { offset: n * 8, size: 8, signed: true });
        Self {
            report_id: None,
            buttons: [bit(0), bit(1), bit(2)],
            x: byte(1),
            y: byte(2),
            wheel: None,
        }
    }

    /// A report as a pointer event; `None` for another report ID's.
    pub fn decode(&self, report: &[u8]) -> Option<Event> {
        let data = match self.report_id {
            Some(id) => {
                let (&first, rest) = report.split_first()?;
                if first != id {
                    return None;
                }
                rest
            }
            None => report,
        };
        let get = |f: &Option<Field>| f.map_or(0, |f| f.get(data));
        let mut buttons = 0u8;
        for (i, b) in self.buttons.iter().enumerate() {
            if get(b) != 0 {
                buttons |= 1 << i;
            }
        }
        Some(Event::Pointer { dx: get(&self.x), dy: get(&self.y), buttons, dz: get(&self.wheel) })
    }
}

/// Why a report descriptor gave no mouse layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// An item runs past the descriptor's end.
    Truncated,
    /// Push nested deeper than the parser keeps.
    TooDeep,
    /// No X and Y of the Generic Desktop page in any input report.
    NoPointer,
}

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u16,
    logical_min: i32,
    size: u32,
    count: u32,
    report_id: u8,
}

const USAGES: usize = 16;
const PAGE_DESKTOP: u16 = 0x01;
const PAGE_BUTTON: u16 = 0x09;
const USAGE_X: u16 = 0x30;
const USAGE_Y: u16 = 0x31;
const USAGE_WHEEL: u16 = 0x38;

/// Lays out a mouse's input report from its report descriptor (HID 1.11
/// section 6.2.2): the fields of the first report ID that has X and Y.
pub fn parse_mouse(desc: &[u8]) -> Result<MouseLayout, ParseError> {
    let mut g = Globals::default();
    let mut stack = [Globals::default(); 4];
    let mut depth = 0;
    // Local items, cleared by each main item: explicit usages (page <<
    // 16 | id when an item gives 4 bytes), or a minimum..maximum range.
    let mut usages = [0u32; USAGES];
    let mut n_usages = 0;
    let (mut usage_min, mut usage_max) = (None::<u32>, None::<u32>);
    // Bit offsets per report ID seen (an ID's fields count from its own 0).
    let mut offsets = [(0u8, 0u32); 8];
    let mut ids = 0;
    let mut chosen: Option<MouseLayout> = None;
    let mut layouts = [(0u8, MouseLayout::default()); 8];
    let mut any_id = false;
    let mut at = 0;
    while at < desc.len() {
        let prefix = desc[at];
        if prefix == 0xfe {
            // A long item: its data size is the next byte.
            let len = usize::from(*desc.get(at + 1).ok_or(ParseError::Truncated)?);
            at += 3 + len;
            continue;
        }
        let size = match prefix & 3 {
            3 => 4,
            n => usize::from(n),
        };
        let data = desc.get(at + 1..at + 1 + size).ok_or(ParseError::Truncated)?;
        at += 1 + size;
        let mut value = 0u32;
        for (i, b) in data.iter().enumerate() {
            value |= u32::from(*b) << (8 * i);
        }
        let signed = match size {
            1 => i32::from(value as u8 as i8),
            2 => i32::from(value as u16 as i16),
            _ => value as i32,
        };
        let (kind, tag) = (prefix >> 2 & 3, prefix >> 4);
        match (kind, tag) {
            // Main: Input.
            (0, 8) => {
                let id = g.report_id;
                let slot = match offsets[..ids].iter().position(|(i, _)| *i == id) {
                    Some(s) => s,
                    None if ids < offsets.len() => {
                        offsets[ids] = (id, 0);
                        layouts[ids] = (id, MouseLayout::default());
                        ids += 1;
                        ids - 1
                    }
                    None => offsets.len() - 1,
                };
                let variable = value & 2 != 0;
                let constant = value & 1 != 0;
                for i in 0..g.count {
                    let offset = offsets[slot].1;
                    offsets[slot].1 += g.size;
                    if constant || !variable || g.size == 0 || g.size > 32 {
                        continue;
                    }
                    let usage = if n_usages > 0 {
                        Some(usages[(i as usize).min(n_usages - 1)])
                    } else {
                        usage_min
                            .map(|min| min + i)
                            .filter(|u| usage_max.is_none_or(|max| *u <= max))
                    };
                    let Some(usage) = usage else { continue };
                    let (page, id16) = if usage > 0xffff {
                        ((usage >> 16) as u16, usage as u16)
                    } else {
                        (g.page, usage as u16)
                    };
                    let Ok(offset) = u16::try_from(offset) else { continue };
                    let field =
                        Some(Field { offset, size: g.size as u8, signed: g.logical_min < 0 });
                    let l = &mut layouts[slot].1;
                    match (page, id16) {
                        (PAGE_BUTTON, 1..=3) => {
                            let b = &mut l.buttons[usize::from(id16 - 1)];
                            if b.is_none() {
                                *b = field;
                            }
                        }
                        (PAGE_DESKTOP, USAGE_X) if l.x.is_none() => l.x = field,
                        (PAGE_DESKTOP, USAGE_Y) if l.y.is_none() => l.y = field,
                        (PAGE_DESKTOP, USAGE_WHEEL) if l.wheel.is_none() => l.wheel = field,
                        _ => {}
                    }
                }
                n_usages = 0;
                (usage_min, usage_max) = (None, None);
            }
            // Main: Output, Feature, Collection, End Collection.
            (0, _) => {
                n_usages = 0;
                (usage_min, usage_max) = (None, None);
            }
            (1, 0) => g.page = value as u16,
            (1, 1) => g.logical_min = signed,
            (1, 7) => g.size = value,
            (1, 8) => {
                g.report_id = value as u8;
                any_id = true;
            }
            (1, 9) => g.count = value,
            (1, 10) => {
                if depth == stack.len() {
                    return Err(ParseError::TooDeep);
                }
                stack[depth] = g;
                depth += 1;
            }
            (1, 11) => {
                if depth > 0 {
                    depth -= 1;
                    g = stack[depth];
                }
            }
            (1, _) => {}
            (2, 0) => {
                if n_usages < USAGES {
                    usages[n_usages] =
                        if size == 4 { value } else { u32::from(g.page) << 16 | value };
                    n_usages += 1;
                }
            }
            (2, 1) => {
                usage_min = Some(if size == 4 { value } else { u32::from(g.page) << 16 | value })
            }
            (2, 2) => {
                usage_max = Some(if size == 4 { value } else { u32::from(g.page) << 16 | value })
            }
            _ => {}
        }
    }
    for (id, l) in &layouts[..ids] {
        if l.x.is_some() && l.y.is_some() {
            chosen = Some(MouseLayout { report_id: any_id.then_some(*id), ..*l });
            break;
        }
    }
    chosen.ok_or(ParseError::NoPointer)
}

/// A boot-protocol keyboard: each report (modifier bits, a reserved
/// byte, up to six usages held) against the one before.
#[derive(Clone, Copy, Debug, Default)]
pub struct Keyboard {
    modifiers: u8,
    keys: [u8; 6],
}

impl Keyboard {
    /// The presses and releases a report makes, in the order a typist's
    /// would: modifiers down, keys up, keys down, modifiers up (so shift+a
    /// gives a capital whatever one report carries).
    pub fn report(&mut self, report: &[u8], mut f: impl FnMut(Event)) {
        let Some(&modifiers) = report.first() else { return };
        let mut keys = [0u8; 6];
        for (i, k) in report.iter().skip(2).take(6).enumerate() {
            keys[i] = *k;
        }
        // ErrorRollOver (1) in every slot: too many keys; the modifiers
        // are still right, the keys are kept as they were.
        let rollover = keys.iter().any(|&k| (1..=3).contains(&k));
        let pressed = modifiers & !self.modifiers;
        let released = self.modifiers & !modifiers;
        for bit in 0..8 {
            if pressed & 1 << bit != 0 {
                f(Event::Key { usage: 0xe0 + bit, down: true });
            }
        }
        if !rollover {
            for &k in self.keys.iter().filter(|&&k| k > 3) {
                if !keys.contains(&k) {
                    f(Event::Key { usage: k, down: false });
                }
            }
            for &k in keys.iter().filter(|&&k| k > 3) {
                if !self.keys.contains(&k) {
                    f(Event::Key { usage: k, down: true });
                }
            }
            self.keys = keys;
        }
        for bit in 0..8 {
            if released & 1 << bit != 0 {
                f(Event::Key { usage: 0xe0 + bit, down: false });
            }
        }
        self.modifiers = modifiers;
    }
}

/// Set 1 scan codes of the Keyboard/Keypad page's usages 0x04..=0x65
/// (0: none; 0x80 marks an E0-prefixed, extended, code).
const SCAN: [u8; 0x62] = [
    // 0x04 a..z
    0x1e,
    0x30,
    0x2e,
    0x20,
    0x12,
    0x21,
    0x22,
    0x23,
    0x17,
    0x24,
    0x25,
    0x26,
    0x32,
    0x31,
    0x18,
    0x19,
    0x10,
    0x13,
    0x1f,
    0x14,
    0x16,
    0x2f,
    0x11,
    0x2d,
    0x15,
    0x2c,
    // 0x1e 1..9, 0
    0x02,
    0x03,
    0x04,
    0x05,
    0x06,
    0x07,
    0x08,
    0x09,
    0x0a,
    0x0b,
    // 0x28 enter, escape, backspace, tab, space, - = [ ] \ (non-US #) ; ' ` , . / caps lock
    0x1c,
    0x01,
    0x0e,
    0x0f,
    0x39,
    0x0c,
    0x0d,
    0x1a,
    0x1b,
    0x2b,
    0x2b,
    0x27,
    0x28,
    0x29,
    0x33,
    0x34,
    0x35,
    0x3a,
    // 0x3a F1..F12
    0x3b,
    0x3c,
    0x3d,
    0x3e,
    0x3f,
    0x40,
    0x41,
    0x42,
    0x43,
    0x44,
    0x57,
    0x58,
    // 0x46 print screen, scroll lock, pause (none: an E1 sequence)
    0x80 | 0x37,
    0x46,
    0,
    // 0x49 insert, home, page up, delete, end, page down, right, left, down, up
    0x80 | 0x52,
    0x80 | 0x47,
    0x80 | 0x49,
    0x80 | 0x53,
    0x80 | 0x4f,
    0x80 | 0x51,
    0x80 | 0x4d,
    0x80 | 0x4b,
    0x80 | 0x50,
    0x80 | 0x48,
    // 0x53 num lock, keypad / * - + enter 1..9 0 .
    0x45,
    0x80 | 0x35,
    0x37,
    0x4a,
    0x4e,
    0x80 | 0x1c,
    0x4f,
    0x50,
    0x51,
    0x4b,
    0x4c,
    0x4d,
    0x47,
    0x48,
    0x49,
    0x52,
    0x53,
    // 0x64 non-US \, application
    0x56,
    0x80 | 0x5d,
];

/// Set 1 scan codes of the modifiers 0xe0..=0xe7: left control, shift,
/// alt, GUI, right control, shift, alt, GUI.
const MODIFIERS: [u8; 8] =
    [0x1d, 0x2a, 0x38, 0x80 | 0x5b, 0x80 | 0x1d, 0x36, 0x80 | 0x38, 0x80 | 0x5c];

/// A Keyboard/Keypad usage as its set 1 scan code and whether it is
/// extended (E0); `None` for one with no set 1 code here.
pub fn scan_code(usage: u8) -> Option<(u8, bool)> {
    let code = match usage {
        0x04..=0x65 => SCAN[usize::from(usage - 4)],
        0xe0..=0xe7 => MODIFIERS[usize::from(usage - 0xe0)],
        _ => 0,
    };
    (code != 0).then_some((code & 0x7f, code & 0x80 != 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn events(k: &mut Keyboard, report: &[u8]) -> Vec<Event> {
        let mut out = Vec::new();
        k.report(report, |e| out.push(e));
        out
    }

    fn key(usage: u8, down: bool) -> Event {
        Event::Key { usage, down }
    }

    #[test]
    fn keyboard_reports_are_diffed() {
        let mut k = Keyboard::default();
        assert_eq!(events(&mut k, &[0, 0, 4, 0, 0, 0, 0, 0]), [key(4, true)]);
        assert_eq!(events(&mut k, &[0, 0, 4, 5, 0, 0, 0, 0]), [key(5, true)]);
        assert_eq!(events(&mut k, &[0, 0, 5, 0, 0, 0, 0, 0]), [key(4, false)]);
        assert_eq!(events(&mut k, &[0; 8]), [key(5, false)]);
        assert_eq!(events(&mut k, &[0; 8]), []);
    }

    #[test]
    fn modifiers_wrap_the_keys() {
        // Shift and a in one report, then both released in one.
        let mut k = Keyboard::default();
        assert_eq!(events(&mut k, &[2, 0, 4, 0, 0, 0, 0, 0]), [key(0xe1, true), key(4, true)]);
        assert_eq!(events(&mut k, &[0; 8]), [key(4, false), key(0xe1, false)]);
    }

    #[test]
    fn rollover_keeps_the_keys() {
        let mut k = Keyboard::default();
        events(&mut k, &[0, 0, 4, 0, 0, 0, 0, 0]);
        assert_eq!(events(&mut k, &[1, 0, 1, 1, 1, 1, 1, 1]), [key(0xe0, true)]);
        assert_eq!(events(&mut k, &[1, 0, 0, 0, 0, 0, 0, 0]), [key(4, false)]);
    }

    #[test]
    fn usages_become_scan_codes() {
        assert_eq!(scan_code(0x04), Some((0x1e, false))); // a
        assert_eq!(scan_code(0x1d), Some((0x2c, false))); // z
        assert_eq!(scan_code(0x27), Some((0x0b, false))); // 0
        assert_eq!(scan_code(0x28), Some((0x1c, false))); // enter
        assert_eq!(scan_code(0x39), Some((0x3a, false))); // caps lock
        assert_eq!(scan_code(0x45), Some((0x58, false))); // F12
        assert_eq!(scan_code(0x48), None); // pause
        assert_eq!(scan_code(0x4f), Some((0x4d, true))); // right
        assert_eq!(scan_code(0x52), Some((0x48, true))); // up
        assert_eq!(scan_code(0x58), Some((0x1c, true))); // keypad enter
        assert_eq!(scan_code(0x63), Some((0x53, false))); // keypad .
        assert_eq!(scan_code(0x65), Some((0x5d, true))); // application
        assert_eq!(scan_code(0xe1), Some((0x2a, false))); // left shift
        assert_eq!(scan_code(0xe6), Some((0x38, true))); // right alt
        assert_eq!(scan_code(0x66), None);
    }

    /// A wheel mouse's descriptor as devices commonly declare it: three
    /// buttons and 5 bits of padding, then X, Y and wheel signed bytes.
    const WHEEL_MOUSE: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29,
        0x03, 0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05,
        0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75,
        0x08, 0x95, 0x03, 0x81, 0x06, 0xc0, 0xc0,
    ];

    #[test]
    fn a_wheel_mouse_is_laid_out() {
        let l = parse_mouse(WHEEL_MOUSE).unwrap();
        let f = |offset, size, signed| Some(Field { offset, size, signed });
        assert_eq!(l.report_id, None);
        assert_eq!(l.buttons, [f(0, 1, false), f(1, 1, false), f(2, 1, false)]);
        assert_eq!((l.x, l.y, l.wheel), (f(8, 8, true), f(16, 8, true), f(24, 8, true)));
        assert_eq!(
            l.decode(&[0b101, 0xfe, 3, 0xff]),
            Some(Event::Pointer { dx: -2, dy: 3, buttons: 0b101, dz: -1 })
        );
    }

    #[test]
    fn report_ids_and_wide_fields() {
        // Report 2 a keyboard-ish constant, report 1 the pointer: buttons
        // 1..5 as a range, 12-bit X and Y, an 8-bit wheel.
        let desc: &[u8] = &[
            0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x02, 0x75, 0x08, 0x95, 0x01, 0x81, 0x01,
            0x85, 0x01, 0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
            0x95, 0x05, 0x81, 0x02, 0x75, 0x03, 0x95, 0x01, 0x81, 0x03, 0x05, 0x01, 0x09, 0x30,
            0x09, 0x31, 0x16, 0x01, 0xf8, 0x26, 0xff, 0x07, 0x75, 0x0c, 0x95, 0x02, 0x81, 0x06,
            0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0xc0,
        ];
        let l = parse_mouse(desc).unwrap();
        assert_eq!(l.report_id, Some(1));
        assert_eq!(l.x, Some(Field { offset: 8, size: 12, signed: true }));
        assert_eq!(l.y, Some(Field { offset: 20, size: 12, signed: true }));
        assert_eq!(l.wheel, Some(Field { offset: 32, size: 8, signed: true }));
        // X -5 (0xffb), Y 300 (0x12c), the right button, wheel +1.
        let report = [1, 0b10, 0xfb, 0xcf, 0x12, 1];
        assert_eq!(l.decode(&report), Some(Event::Pointer { dx: -5, dy: 300, buttons: 2, dz: 1 }));
        assert_eq!(l.decode(&[2, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn descriptors_without_a_pointer_are_refused() {
        assert_eq!(
            parse_mouse(&[0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0xc0]),
            Err(ParseError::NoPointer)
        );
        assert_eq!(parse_mouse(&[0x05, 0x01, 0x26, 0xff]), Err(ParseError::Truncated));
    }

    #[test]
    fn the_boot_layout() {
        let l = MouseLayout::boot();
        assert_eq!(
            l.decode(&[1, 5, 0xfb]),
            Some(Event::Pointer { dx: 5, dy: -5, buttons: 1, dz: 0 })
        );
    }
}
