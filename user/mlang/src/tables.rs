//! The code-page tables mlang answers from: a code page's
//! FONTSIGNATURE bit, the bits back to a code page, and the script a
//! code page writes.
//!
//! One row per code page that has a bit or a script, as Windows 11 24H2
//! answers for it (`user/mlang_test/main.c`). The table is the whole of
//! what these three methods know: it names code pages gala's NLS cannot
//! convert yet, which is right -- a program asks which code page covers
//! a script before it asks anyone to convert anything, and the answer
//! does not depend on who can.

/// The FONTSIGNATURE code-page bits (`fsCsb[0]`), in the order the bits
/// run: 1250..1258 are bits 1..8 with Latin 1 first.
pub const FS_LATIN1: u32 = 0x0000_0001;
pub const FS_LATIN2: u32 = 0x0000_0002;
pub const FS_CYRILLIC: u32 = 0x0000_0004;
pub const FS_GREEK: u32 = 0x0000_0008;
pub const FS_TURKISH: u32 = 0x0000_0010;
pub const FS_HEBREW: u32 = 0x0000_0020;
pub const FS_ARABIC: u32 = 0x0000_0040;
pub const FS_BALTIC: u32 = 0x0000_0080;
pub const FS_VIETNAMESE: u32 = 0x0000_0100;
pub const FS_THAI: u32 = 0x0001_0000;
pub const FS_JISJAPAN: u32 = 0x0002_0000;
pub const FS_CHINESESIMP: u32 = 0x0004_0000;
pub const FS_WANSUNG: u32 = 0x0008_0000;
pub const FS_CHINESETRAD: u32 = 0x0010_0000;

/// The SCRIPT_IDs the rows use (mlang's `SCRIPT_ID` enumeration).
const SID_ASCII_LATIN: u8 = 3;
const SID_GREEK: u8 = 5;
const SID_CYRILLIC: u8 = 6;
const SID_HEBREW: u8 = 8;
const SID_ARABIC: u8 = 9;
const SID_THAI: u8 = 19;
const SID_HANGUL: u8 = 23;
const SID_KANA: u8 = 24;
const SID_BOPOMOFO: u8 = 25;
const SID_HAN: u8 = 26;

/// A code page, the FONTSIGNATURE bit it is (0: none), and the script it
/// writes (`None`: none). 437 has a script and no bit, which is why the
/// two are one table and not two.
struct Row {
    page: u32,
    bit: u32,
    script: Option<u8>,
}

static ROWS: &[Row] = &[
    Row { page: 437, bit: 0, script: Some(SID_ASCII_LATIN) },
    Row { page: 874, bit: FS_THAI, script: Some(SID_THAI) },
    Row { page: 932, bit: FS_JISJAPAN, script: Some(SID_KANA) },
    Row { page: 936, bit: FS_CHINESESIMP, script: Some(SID_HAN) },
    Row { page: 949, bit: FS_WANSUNG, script: Some(SID_HANGUL) },
    Row { page: 950, bit: FS_CHINESETRAD, script: Some(SID_BOPOMOFO) },
    Row { page: 1250, bit: FS_LATIN2, script: Some(SID_ASCII_LATIN) },
    Row { page: 1251, bit: FS_CYRILLIC, script: Some(SID_CYRILLIC) },
    Row { page: 1252, bit: FS_LATIN1, script: Some(SID_ASCII_LATIN) },
    Row { page: 1253, bit: FS_GREEK, script: Some(SID_GREEK) },
    Row { page: 1254, bit: FS_TURKISH, script: Some(SID_ASCII_LATIN) },
    Row { page: 1255, bit: FS_HEBREW, script: Some(SID_HEBREW) },
    Row { page: 1256, bit: FS_ARABIC, script: Some(SID_ARABIC) },
    Row { page: 1257, bit: FS_BALTIC, script: Some(SID_ASCII_LATIN) },
    Row { page: 1258, bit: FS_VIETNAMESE, script: Some(SID_ASCII_LATIN) },
];

fn row(page: u32) -> Option<&'static Row> {
    ROWS.iter().find(|r| r.page == page)
}

/// CodePageToCodePages: the one code-page bit a code page is, or `None`
/// for a code page that is no bit at all (437, 850, UTF-8, the ISO and
/// EUC pages, a page that does not exist).
pub fn code_pages_of(page: u32) -> Option<u32> {
    match row(page) {
        Some(r) if r.bit != 0 => Some(r.bit),
        _ => None,
    }
}

/// CodePagesToCodePage: the code page a mask of bits stands for. The
/// default wins when its own bit is in the mask; otherwise the lowest
/// bit that names a code page does; and with no bit to go by -- an empty
/// mask, or one of bits that name no code page (FS_SYMBOL among them) --
/// the answer is the default, or `None` when there is not even that.
pub fn code_page_of(mask: u32, default: u32) -> Option<u32> {
    if default != 0 && code_pages_of(default).is_some_and(|bit| mask & bit != 0) {
        return Some(default);
    }
    let lowest = ROWS
        .iter()
        .filter(|r| r.bit != 0 && mask & r.bit != 0)
        .min_by_key(|r| r.bit)
        .map(|r| r.page);
    lowest.or(if default == 0 { None } else { Some(default) })
}

/// CodePageToScriptID: the script a code page writes, or `None` for one
/// that writes none (0, UTF-16, UTF-8, a page that does not exist).
pub fn script_of(page: u32) -> Option<u8> {
    row(page).and_then(|r| r.script)
}
