//! The object CMultiLanguage serves: what each interface comes back on,
//! what its references count, and which of its methods gala answers.
//!
//! IMultiLanguage is the object's own identity -- it and IUnknown come
//! back on the object's own pointer, as Windows' object does. Every
//! other interface comes back on a tear-off of its own, with its own
//! reference count and one counted reference on the object, so the
//! object outlives every tear-off; a tear-off's QueryInterface asks the
//! object, so IUnknown is the same pointer whichever interface it is
//! asked through, which is the rule COM is built on and what
//! `mlang_test` checks.
//!
//! The methods that answer out of the code-page tables are in `tables`
//! and the ones that answer out of the font a DC has selected are in
//! `fonts`. The rest are E_NOTIMPL with a ledger row naming the
//! interface and the method, because what they convert, enumerate or
//! describe needs the per-code-page tables gala's NLS has not got
//! (kernel32 knows 65001, 20127, 1252 and 437, and converts each of
//! them as UTF-8): a string converted, or a code page described, out of
//! a table that is not there would be wrong without saying so, and a
//! program is better told.

use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering, fence};

use gala_lock::Lock;

use crate::{
    E_FAIL, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_IUNKNOWN,
    LEDGER_OTHER, S_OK, VTable, fonts, guid, not_implemented, put, reached, record, record_iid,
    tables, try_box,
};

const IID_IMULTILANGUAGE: Guid =
    guid(0x275c_23e1, 0x3747, 0x11d0, [0x9f, 0xea, 0x00, 0xaa, 0x00, 0x3f, 0x86, 0x46]);
const IID_IMULTILANGUAGE2: Guid =
    guid(0xdccf_c164, 0x2b38, 0x11d2, [0xb7, 0xec, 0x00, 0xc0, 0x4f, 0x8f, 0x5d, 0x9a]);
const IID_IMULTILANGUAGE3: Guid =
    guid(0x4e58_68ab, 0xb157, 0x4623, [0x9a, 0xcc, 0x6a, 0x1d, 0x9c, 0xae, 0xbe, 0x04]);
const IID_IMLANGCODEPAGES: Guid =
    guid(0x359f_3443, 0xbd4a, 0x11d0, [0xb1, 0x88, 0x00, 0xaa, 0x00, 0x38, 0xc9, 0x69]);
const IID_IMLANGFONTLINK: Guid =
    guid(0x359f_3441, 0xbd4a, 0x11d0, [0xb1, 0x88, 0x00, 0xaa, 0x00, 0x38, 0xc9, 0x69]);
const IID_IMLANGFONTLINK2: Guid =
    guid(0xdccf_c162, 0x2b38, 0x11d2, [0xb7, 0xec, 0x00, 0xc0, 0x4f, 0x8f, 0x5d, 0x9a]);
const IID_IMLANGLINEBREAKCONSOLE: Guid =
    guid(0xf5be_2ee1, 0xbfd7, 0x11d0, [0xb1, 0x88, 0x00, 0xaa, 0x00, 0x38, 0xc9, 0x69]);

/// The object. Its first word is the IMultiLanguage vtable, which is
/// the pointer a program holding IMultiLanguage (or IUnknown) has.
#[repr(C)]
pub struct Object {
    vtable: *const VTable<18>,
    refs: AtomicU32,
    /// Over `fonts`, because the class is registered ThreadingModel
    /// Both: two threads of one process may hold the same object and
    /// map fonts on it at once.
    lock: Lock,
    /// The fonts MapFont has handed out and nobody has released, as
    /// handle values.
    fonts: Vec<usize>,
}

/// An interface that is not the object's own identity: its vtable, its
/// own references, and the object it belongs to.
#[repr(C)]
struct TearOff {
    vtable: *const (),
    refs: AtomicU32,
    owner: *mut Object,
}

/// The object a method was called on, whichever interface it came in
/// through: a tear-off's third word is its owner, and the object's own
/// vtable makes `this` the object itself.
fn object_of(this: *mut c_void, tear_off: bool) -> *mut Object {
    if tear_off {
        // SAFETY: `this` is a tear-off this DLL made.
        unsafe { (*this.cast::<TearOff>()).owner }
    } else {
        this.cast::<Object>()
    }
}

/// Holds the object's lock over its font table. The table is touched
/// only here, and nothing is called with the lock held
/// (`user/gala_lock`): the deletes happen after `take_fonts` has handed
/// the handles over.
fn with_fonts<R>(object: *mut Object, f: impl FnOnce(&mut Vec<usize>) -> R) -> R {
    // SAFETY: a live object -- its caller holds a reference on it.
    let lock = unsafe { &(*object).lock };
    let _held = lock.hold();
    // SAFETY: the lock is held, so this is the only live reference to
    // the table.
    f(unsafe { &mut (*object).fonts })
}

/// Remembers a font MapFont made; false when the heap had nothing left,
/// which is the one case where the handle must not be handed out.
pub fn remember_font(object: *mut Object, font: *mut c_void) -> bool {
    with_fonts(object, |fonts| {
        if fonts.try_reserve(1).is_err() {
            record(b"MapFont(out of memory)", LEDGER_OTHER, 0);
            return false;
        }
        fonts.push(font as usize);
        true
    })
}

/// Takes a font out of the table; false when mlang never mapped it (the
/// program's own font, or one released already).
pub fn forget_font(object: *mut Object, font: *mut c_void) -> bool {
    with_fonts(object, |fonts| match fonts.iter().position(|&f| f == font as usize) {
        Some(at) => {
            fonts.swap_remove(at);
            true
        }
        None => false,
    })
}

/// Empties the table and hands the handles to the caller to delete.
pub fn take_fonts(object: *mut Object) -> Vec<usize> {
    with_fonts(object, core::mem::take)
}

// ---- The counted references ----

extern "system" fn object_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: a live object.
    let object = unsafe { &*this.cast::<Object>() };
    object.refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn object_release(this: *mut c_void) -> u32 {
    let object = this.cast::<Object>();
    // SAFETY: a live object. The release orders everything this thread
    // did to the object before the last reference goes.
    let left = unsafe { (*object).refs.fetch_sub(1, Ordering::Release) } - 1;
    if left == 0 {
        // Against the releases of the other threads that held it.
        fence(Ordering::Acquire);
        fonts::reset(object);
        // SAFETY: the last reference is gone, the object came from
        // `try_box` and nothing reaches it after this.
        unsafe { crate::free_box(object) };
    }
    left
}

extern "system" fn tear_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // Every interface of this object is asked of the object itself, so
    // that one object has one IUnknown.
    object_query(object_of(this, true).cast(), riid, out)
}

extern "system" fn tear_add_ref(this: *mut c_void) -> u32 {
    // SAFETY: a live tear-off.
    let tear_off = unsafe { &*this.cast::<TearOff>() };
    tear_off.refs.fetch_add(1, Ordering::Relaxed) + 1
}

extern "system" fn tear_release(this: *mut c_void) -> u32 {
    let tear_off = this.cast::<TearOff>();
    // SAFETY: a live tear-off.
    let left = unsafe { (*tear_off).refs.fetch_sub(1, Ordering::Release) } - 1;
    if left == 0 {
        fence(Ordering::Acquire);
        // SAFETY: the last reference to this tear-off.
        let owner = unsafe { (*tear_off).owner };
        // SAFETY: from `try_box`, and nothing reaches it after this.
        unsafe { crate::free_box(tear_off) };
        // The reference the tear-off held on the object goes with it.
        object_release(owner.cast());
    }
    left
}

// ---- The methods gala answers ----

/// CodePageToCodePages(uCodePage, pdwCodePages), on IMLangCodePages and
/// both font links.
extern "system" fn code_page_to_code_pages(
    _this: *mut c_void,
    code_page: u32,
    out: *mut u32,
) -> HResult {
    reached(crate::REACHED_CODE_PAGE_TO_CODE_PAGES, b"CodePageToCodePages");
    put(out, 0);
    let Some(bits) = tables::code_pages_of(code_page) else { return E_FAIL };
    put(out, bits);
    S_OK
}

/// CodePagesToCodePage(dwCodePages, uDefaultCodePage, puCodePage).
extern "system" fn code_pages_to_code_page(
    _this: *mut c_void,
    code_pages: u32,
    default: u32,
    out: *mut u32,
) -> HResult {
    reached(crate::REACHED_CODE_PAGES_TO_CODE_PAGE, b"CodePagesToCodePage");
    put(out, 0);
    let Some(page) = tables::code_page_of(code_pages, default) else { return E_FAIL };
    put(out, page);
    S_OK
}

/// CodePageToScriptID(uiCodePage, pSid). A code page with no script
/// leaves the caller's SCRIPT_ID as it was, which is what Windows does
/// and so what a caller that reads it without looking at the HRESULT
/// sees.
extern "system" fn code_page_to_script_id(
    _this: *mut c_void,
    code_page: u32,
    out: *mut u8,
) -> HResult {
    reached(crate::REACHED_CODE_PAGE_TO_SCRIPT_ID, b"CodePageToScriptID");
    if out.is_null() {
        return E_INVALIDARG;
    }
    let Some(script) = tables::script_of(code_page) else { return E_FAIL };
    put(out, script);
    S_OK
}

extern "system" fn get_font_code_pages(
    _this: *mut c_void,
    dc: *mut c_void,
    font: *mut c_void,
    out: *mut u32,
) -> HResult {
    fonts::get_font_code_pages(dc, font, out)
}

/// IMLangFontLink2's MapFont(hDC, dwCodePages, chSrc, pFont).
extern "system" fn map_font2(
    this: *mut c_void,
    dc: *mut c_void,
    code_pages: u32,
    character: u16,
    out: *mut *mut c_void,
) -> HResult {
    fonts::map_font(object_of(this, true), dc, code_pages, character, out)
}

extern "system" fn release_font(this: *mut c_void, font: *mut c_void) -> HResult {
    fonts::release_font(object_of(this, true), font)
}

extern "system" fn reset_font_mapping(this: *mut c_void) -> HResult {
    fonts::reset_font_mapping(object_of(this, true))
}

// ---- The methods gala does not ----

/// A method that is not implemented: its arguments are not read, its
/// row names the interface it was asked of, and the caller gets
/// E_NOTIMPL. The argument list is the method's arity, which the 32-bit
/// build's stdcall needs right -- a thunk pops its own arguments.
macro_rules! notimpl {
    ($($name:ident($($arg:ident),*) = $method:expr;)*) => {$(
        extern "system" fn $name(_this: *mut c_void $(, $arg: usize)*) -> HResult {
            $(let _ = $arg;)*
            not_implemented($method.as_bytes())
        }
    )*};
}

/// An interface whose every method is not implemented: the thunks and
/// the vtable, in slot order, with the three IUnknown slots handed in.
macro_rules! notimpl_iface {
    (
        $module:ident, $slots:literal, [$query:ident, $add_ref:ident, $release:ident],
        { $($name:ident($($arg:ident),*) = $method:expr;)* }
    ) => {
        mod $module {
            use super::*;
            notimpl! { $($name($($arg),*) = $method;)* }
            pub static VTABLE: VTable<$slots> = VTable([
                $query as *const (),
                $add_ref as *const (),
                $release as *const (),
                $($name as *const (),)*
            ]);
        }
    };
}

/// IMultiLanguage2's twenty-seven methods, which IMultiLanguage3 shares
/// and adds two of its own to. The row names the interface the program
/// holds, so the list is instantiated once per interface.
macro_rules! multi_language2 {
    ($module:ident, $slots:literal, $prefix:literal, { $($extra:tt)* }) => {
        notimpl_iface!($module, $slots, [tear_query, tear_add_ref, tear_release], {
            get_number_of_code_page_info(a1) = concat!($prefix, "::GetNumberOfCodePageInfo");
            get_code_page_info(a1, a2, a3) = concat!($prefix, "::GetCodePageInfo");
            get_family_code_page(a1, a2) = concat!($prefix, "::GetFamilyCodePage");
            enum_code_pages(a1, a2, a3) = concat!($prefix, "::EnumCodePages");
            get_charset_info(a1, a2) = concat!($prefix, "::GetCharsetInfo");
            is_convertible(a1, a2) = concat!($prefix, "::IsConvertible");
            convert_string(a1, a2, a3, a4, a5, a6, a7) = concat!($prefix, "::ConvertString");
            convert_string_to_unicode(a1, a2, a3, a4, a5, a6) =
                concat!($prefix, "::ConvertStringToUnicode");
            convert_string_from_unicode(a1, a2, a3, a4, a5, a6) =
                concat!($prefix, "::ConvertStringFromUnicode");
            convert_string_reset() = concat!($prefix, "::ConvertStringReset");
            get_rfc1766_from_lcid(a1, a2) = concat!($prefix, "::GetRfc1766FromLcid");
            get_lcid_from_rfc1766(a1, a2) = concat!($prefix, "::GetLcidFromRfc1766");
            enum_rfc1766(a1, a2) = concat!($prefix, "::EnumRfc1766");
            get_rfc1766_info(a1, a2, a3) = concat!($prefix, "::GetRfc1766Info");
            create_convert_charset(a1, a2, a3, a4) = concat!($prefix, "::CreateConvertCharset");
            convert_string_in_istream(a1, a2, a3, a4, a5, a6, a7) =
                concat!($prefix, "::ConvertStringInIStream");
            convert_string_to_unicode_ex(a1, a2, a3, a4, a5, a6, a7, a8) =
                concat!($prefix, "::ConvertStringToUnicodeEx");
            convert_string_from_unicode_ex(a1, a2, a3, a4, a5, a6, a7, a8) =
                concat!($prefix, "::ConvertStringFromUnicodeEx");
            detect_codepage_in_istream(a1, a2, a3, a4, a5) =
                concat!($prefix, "::DetectCodepageInIStream");
            detect_input_codepage(a1, a2, a3, a4, a5, a6) =
                concat!($prefix, "::DetectInputCodepage");
            validate_code_page(a1, a2) = concat!($prefix, "::ValidateCodePage");
            get_code_page_description(a1, a2, a3, a4) =
                concat!($prefix, "::GetCodePageDescription");
            is_code_page_installable(a1) = concat!($prefix, "::IsCodePageInstallable");
            set_mime_db_source(a1) = concat!($prefix, "::SetMimeDBSource");
            get_number_of_scripts(a1) = concat!($prefix, "::GetNumberOfScripts");
            enum_scripts(a1, a2, a3) = concat!($prefix, "::EnumScripts");
            validate_code_page_ex(a1, a2, a3) = concat!($prefix, "::ValidateCodePageEx");
            $($extra)*
        });
    };
}

// IMultiLanguage, the object's own identity: its three IUnknown slots
// are the object's own. Four of its methods take one argument fewer
// than IMultiLanguage2's of the same name, so the list is its own.
notimpl_iface!(ml1, 18, [object_query, object_add_ref, object_release], {
    get_number_of_code_page_info(a1) = "IMultiLanguage::GetNumberOfCodePageInfo";
    get_code_page_info(a1, a2) = "IMultiLanguage::GetCodePageInfo";
    get_family_code_page(a1, a2) = "IMultiLanguage::GetFamilyCodePage";
    enum_code_pages(a1, a2) = "IMultiLanguage::EnumCodePages";
    get_charset_info(a1, a2) = "IMultiLanguage::GetCharsetInfo";
    is_convertible(a1, a2) = "IMultiLanguage::IsConvertible";
    convert_string(a1, a2, a3, a4, a5, a6, a7) = "IMultiLanguage::ConvertString";
    convert_string_to_unicode(a1, a2, a3, a4, a5, a6) = "IMultiLanguage::ConvertStringToUnicode";
    convert_string_from_unicode(a1, a2, a3, a4, a5, a6) =
        "IMultiLanguage::ConvertStringFromUnicode";
    convert_string_reset() = "IMultiLanguage::ConvertStringReset";
    get_rfc1766_from_lcid(a1, a2) = "IMultiLanguage::GetRfc1766FromLcid";
    get_lcid_from_rfc1766(a1, a2) = "IMultiLanguage::GetLcidFromRfc1766";
    enum_rfc1766(a1) = "IMultiLanguage::EnumRfc1766";
    get_rfc1766_info(a1, a2) = "IMultiLanguage::GetRfc1766Info";
    create_convert_charset(a1, a2, a3, a4) = "IMultiLanguage::CreateConvertCharset";
});

multi_language2!(ml2, 30, "IMultiLanguage2", {});
multi_language2!(ml3, 32, "IMultiLanguage3", {
    detect_outbound_code_page(a1, a2, a3, a4, a5, a6, a7, a8) =
        "IMultiLanguage3::DetectOutboundCodePage";
    detect_outbound_code_page_in_istream(a1, a2, a3, a4, a5, a6, a7) =
        "IMultiLanguage3::DetectOutboundCodePageInIStream";
});

notimpl_iface!(line_break, 6, [tear_query, tear_add_ref, tear_release], {
    break_line_ml(a1, a2, a3, a4, a5, a6, a7) = "IMLangLineBreakConsole::BreakLineML";
    break_line_w(a1, a2, a3, a4, a5, a6) = "IMLangLineBreakConsole::BreakLineW";
    break_line_a(a1, a2, a3, a4, a5, a6, a7) = "IMLangLineBreakConsole::BreakLineA";
});

// The methods of the code-page and font-link interfaces gala does not
// answer, one thunk per interface they are asked of so that a row says
// which interface the program holds. The two that walk a string need a
// code page's encode table to answer at all; the font links' Unicode
// ranges and script font info need the font tables behind them;
// IMLangFontLink::MapFont takes a source font where IMLangFontLink2's
// takes a character, and that shape was never measured.
notimpl! {
    codepages_get_char_code_pages(a1, a2) = "IMLangCodePages::GetCharCodePages";
    codepages_get_str_code_pages(a1, a2, a3, a4, a5) = "IMLangCodePages::GetStrCodePages";
    fontlink_get_char_code_pages(a1, a2) = "IMLangFontLink::GetCharCodePages";
    fontlink_get_str_code_pages(a1, a2, a3, a4, a5) = "IMLangFontLink::GetStrCodePages";
    fontlink_map_font(a1, a2, a3, a4) = "IMLangFontLink::MapFont(a source font)";
    fontlink2_get_char_code_pages(a1, a2) = "IMLangFontLink2::GetCharCodePages";
    fontlink2_get_str_code_pages(a1, a2, a3, a4, a5) = "IMLangFontLink2::GetStrCodePages";
    fontlink2_get_font_unicode_ranges(a1, a2, a3) = "IMLangFontLink2::GetFontUnicodeRanges";
    fontlink2_get_script_font_info(a1, a2, a3, a4) = "IMLangFontLink2::GetScriptFontInfo";
}

static CODEPAGES_VTABLE: VTable<7> = VTable([
    tear_query as *const (),
    tear_add_ref as *const (),
    tear_release as *const (),
    codepages_get_char_code_pages as *const (),
    codepages_get_str_code_pages as *const (),
    code_page_to_code_pages as *const (),
    code_pages_to_code_page as *const (),
]);

static FONTLINK_VTABLE: VTable<11> = VTable([
    tear_query as *const (),
    tear_add_ref as *const (),
    tear_release as *const (),
    fontlink_get_char_code_pages as *const (),
    fontlink_get_str_code_pages as *const (),
    code_page_to_code_pages as *const (),
    code_pages_to_code_page as *const (),
    get_font_code_pages as *const (),
    fontlink_map_font as *const (),
    release_font as *const (),
    reset_font_mapping as *const (),
]);

static FONTLINK2_VTABLE: VTable<14> = VTable([
    tear_query as *const (),
    tear_add_ref as *const (),
    tear_release as *const (),
    fontlink2_get_char_code_pages as *const (),
    fontlink2_get_str_code_pages as *const (),
    code_page_to_code_pages as *const (),
    code_pages_to_code_page as *const (),
    get_font_code_pages as *const (),
    release_font as *const (),
    reset_font_mapping as *const (),
    map_font2 as *const (),
    fontlink2_get_font_unicode_ranges as *const (),
    fontlink2_get_script_font_info as *const (),
    code_page_to_script_id as *const (),
]);

// ---- QueryInterface and the making of the object ----

/// The object's QueryInterface: IUnknown and IMultiLanguage are the
/// object itself, every other interface gala serves comes back on a
/// tear-off of its own, and anything else is recorded and refused with
/// the caller's pointer left NULL.
extern "system" fn object_query(
    this: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    put(out, ptr::null_mut());
    // SAFETY: the caller's IID.
    let iid = unsafe { *riid };
    if iid == IID_IUNKNOWN || iid == IID_IMULTILANGUAGE {
        object_add_ref(this);
        put(out, this);
        return S_OK;
    }
    let vtable: *const () = if iid == IID_IMULTILANGUAGE2 {
        (&ml2::VTABLE as *const VTable<30>).cast()
    } else if iid == IID_IMULTILANGUAGE3 {
        (&ml3::VTABLE as *const VTable<32>).cast()
    } else if iid == IID_IMLANGCODEPAGES {
        (&CODEPAGES_VTABLE as *const VTable<7>).cast()
    } else if iid == IID_IMLANGFONTLINK {
        (&FONTLINK_VTABLE as *const VTable<11>).cast()
    } else if iid == IID_IMLANGFONTLINK2 {
        (&FONTLINK2_VTABLE as *const VTable<14>).cast()
    } else if iid == IID_IMLANGLINEBREAKCONSOLE {
        (&line_break::VTABLE as *const VTable<6>).cast()
    } else {
        record_iid("QI? CMultiLanguage", &iid);
        return E_NOINTERFACE;
    };
    let Some(tear_off) =
        try_box(TearOff { vtable, refs: AtomicU32::new(1), owner: this.cast::<Object>() })
    else {
        record(b"QueryInterface(out of memory)", LEDGER_OTHER, 0);
        return E_OUTOFMEMORY;
    };
    // The tear-off holds the object while it lives.
    object_add_ref(this);
    put(out, tear_off.cast::<c_void>());
    S_OK
}

/// A new object, asked for the interface the caller wants (ole32's
/// CoCreateInstance, through the class object's CreateInstance). Each
/// creation is an object of its own, as Windows' class object makes
/// one: two creations are never the same pointer.
pub fn new_object(riid: *const Guid, out: *mut *mut c_void) -> HResult {
    if riid.is_null() {
        return E_POINTER;
    }
    let Some(object) = try_box(Object {
        vtable: &ml1::VTABLE,
        refs: AtomicU32::new(1),
        lock: Lock::new(b"mlang.dll!mapped fonts"),
        fonts: Vec::new(),
    }) else {
        record(b"CMultiLanguage(out of memory)", LEDGER_OTHER, 0);
        return E_OUTOFMEMORY;
    };
    reached(crate::REACHED_CREATED, b"CMultiLanguage(created)");
    let hr = object_query(object.cast::<c_void>(), riid, out);
    // The creation's own reference: what the caller holds now is the
    // interface it asked for, and a refused interface frees the object.
    object_release(object.cast::<c_void>());
    hr
}
