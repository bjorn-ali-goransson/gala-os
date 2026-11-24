//! webservices.dll's XML reader, which the Office Deployment Tool's
//! setup.exe delay-loads to parse the Office configuration service's
//! answer (ROADMAP OF). setup.exe's delay imports are the reader and
//! error subset alone -- no channel, no service proxy, no security
//! binding: WsCreateError, WsCreateHeap, WsCreateReader, WsCreateWriter,
//! WsFillReader, WsFreeError, WsFreeHeap, WsFreeReader, WsFreeWriter,
//! WsGetErrorProperty, WsGetErrorString, WsReadStartElement,
//! WsReadToStartElement, WsReadType and WsSetInput.
//!
//! The declarations and constants are those of the Windows SDK's
//! WebServices.h (10.0.26100.0, the pinned build's), and every expected
//! value is what real Windows answers (this program's run on CI's
//! runner and on the owner's machine).
//!
//! The same source builds `wow64_webservices_test`, the 32-bit (x86)
//! program. Prints `webservices_test: ok`, or `<name>: check N got X
//! (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use ntrt as _;

#[cfg(not(target_arch = "x86"))]
const NAME: &str = "webservices_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_webservices_test";

const CRLF: &[u8] = &[13, 10];

type Handle = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

// WebServices.h: the reader and error functions setup.exe imports.
#[cfg_attr(
    target_arch = "x86",
    link(name = "webservices", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "webservices", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WsCreateHeap(
        max_size: usize,
        trim_size: usize,
        properties: *const c_void,
        property_count: u32,
        heap: *mut *mut c_void,
        error: *mut c_void,
    ) -> i32;
    fn WsFreeHeap(heap: *mut c_void);
    fn WsCreateError(
        properties: *const c_void,
        property_count: u32,
        error: *mut *mut c_void,
    ) -> i32;
    fn WsFreeError(error: *mut c_void);
    fn WsGetErrorProperty(
        error: *mut c_void,
        id: u32,
        buffer: *mut c_void,
        buffer_size: u32,
    ) -> i32;
    fn WsGetErrorString(error: *mut c_void, index: u32, string: *mut WsString) -> i32;
    fn WsCreateReader(
        properties: *const c_void,
        property_count: u32,
        reader: *mut *mut c_void,
        error: *mut c_void,
    ) -> i32;
    fn WsFreeReader(reader: *mut c_void);
    fn WsCreateWriter(
        properties: *const c_void,
        property_count: u32,
        writer: *mut *mut c_void,
        error: *mut c_void,
    ) -> i32;
    fn WsFreeWriter(writer: *mut c_void);
    fn WsSetInput(
        reader: *mut c_void,
        encoding: *const c_void,
        input: *const c_void,
        properties: *const c_void,
        property_count: u32,
        error: *mut c_void,
    ) -> i32;
    fn WsFillReader(
        reader: *mut c_void,
        min_size: u32,
        async_context: *const c_void,
        error: *mut c_void,
    ) -> i32;
    fn WsReadToStartElement(
        reader: *mut c_void,
        local_name: *const WsXmlString,
        ns: *const WsXmlString,
        found: *mut i32,
        error: *mut c_void,
    ) -> i32;
    fn WsReadStartElement(reader: *mut c_void, error: *mut c_void) -> i32;
    fn WsReadEndElement(reader: *mut c_void, error: *mut c_void) -> i32;
    fn WsGetReaderProperty(
        reader: *mut c_void,
        id: u32,
        value: *mut c_void,
        value_size: u32,
        error: *mut c_void,
    ) -> i32;
    fn WsReadType(
        reader: *mut c_void,
        type_mapping: u32,
        ws_type: u32,
        type_description: *const c_void,
        read_option: u32,
        heap: *mut c_void,
        value: *mut c_void,
        value_size: u32,
        error: *mut c_void,
    ) -> i32;
}

/// WebServices.h's WS_STRING: a UTF-16 run and its length in characters.
#[repr(C)]
#[derive(Clone, Copy)]
struct WsString {
    length: u32,
    chars: *mut u16,
}

/// WebServices.h's WS_XML_STRING: UTF-8 bytes, with the dictionary a
/// non-dictionary string leaves NULL.
#[repr(C)]
struct WsXmlString {
    length: u32,
    bytes: *const u8,
    dictionary: *const c_void,
    id: u32,
}

impl WsXmlString {
    const fn new(bytes: &'static [u8]) -> Self {
        Self { length: bytes.len() as u32, bytes: bytes.as_ptr(), dictionary: null(), id: 0 }
    }
}

/// WebServices.h's WS_XML_READER_TEXT_ENCODING: the base
/// WS_XML_READER_ENCODING (its type) and the character set.
#[repr(C)]
struct WsXmlReaderTextEncoding {
    encoding_type: u32,
    char_set: u32,
}

/// WebServices.h's WS_XML_READER_PROPERTY: which setting, a pointer to
/// its value and the value's size.
#[repr(C)]
struct WsXmlReaderProperty {
    id: u32,
    value: *mut c_void,
    value_size: u32,
}

/// WebServices.h's WS_XML_READER_BUFFER_INPUT: the base
/// WS_XML_READER_INPUT (its type) and the bytes to read.
#[repr(C)]
struct WsXmlReaderBufferInput {
    input_type: u32,
    encoded_data: *const c_void,
    encoded_data_size: u32,
}

// WebServices.h's enumerations, the members this program uses.
const WS_XML_READER_INPUT_TYPE_BUFFER: u32 = 1;
const WS_XML_READER_ENCODING_TYPE_TEXT: u32 = 1;
const WS_CHARSET_UTF8: u32 = 1;
const WS_ERROR_PROPERTY_STRING_COUNT: u32 = 0;
const WS_ELEMENT_TYPE_MAPPING: u32 = 1;
const WS_ELEMENT_CONTENT_TYPE_MAPPING: u32 = 3;
const WS_XML_READER_PROPERTY_MAX_DEPTH: u32 = 0;
const WS_XML_READER_PROPERTY_ALLOW_FRAGMENT: u32 = 1;
const WS_XML_READER_PROPERTY_MAX_ATTRIBUTES: u32 = 2;
const WS_XML_READER_PROPERTY_READ_DECLARATION: u32 = 3;
const WS_XML_READER_PROPERTY_CHARSET: u32 = 4;
const WS_XML_READER_PROPERTY_ROW: u32 = 5;
const WS_XML_READER_PROPERTY_COLUMN: u32 = 6;
const WS_XML_READER_PROPERTY_UTF8_TRIM_SIZE: u32 = 7;
const WS_XML_READER_PROPERTY_STREAM_BUFFER_SIZE: u32 = 8;
const WS_XML_READER_PROPERTY_IN_ATTRIBUTE: u32 = 9;
const WS_XML_READER_PROPERTY_ALLOW_INVALID_REFS: u32 = 13;
const WS_XML_READER_PROPERTY_MAX_NAMESPACES: u32 = 14;
const WS_UINT32_TYPE: u32 = 7;
const WS_WSZ_TYPE: u32 = 17;
const WS_STRUCT_TYPE: u32 = 26;
const WS_ATTRIBUTE_FIELD_MAPPING: u32 = 1;
const WS_ELEMENT_FIELD_MAPPING: u32 = 2;
const WS_REPEATING_ELEMENT_FIELD_MAPPING: u32 = 3;
const WS_TEXT_FIELD_MAPPING: u32 = 4;
const WS_FIELD_POINTER: u32 = 0x1;
const WS_FIELD_OPTIONAL: u32 = 0x2;
const WS_STRUCT_IGNORE_TRAILING_ELEMENT_CONTENT: u32 = 0x2;
const WS_STRUCT_IGNORE_UNHANDLED_ATTRIBUTES: u32 = 0x4;
const WS_READ_REQUIRED_VALUE: u32 = 1;
const WS_READ_REQUIRED_POINTER: u32 = 2;

const S_OK: i32 = 0;

/// One line of output, written with a single WriteFile: the kernel log
/// shares the serial port, so a line built from several writes can be
/// cut in half by a log line (WORKFLOW "Test programs").
struct Line {
    buf: [u8; 240],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 240], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(mut self, v: u64) -> Self {
        let mut digits = [0u8; 20];
        let mut n = 0;
        let mut v = v;
        loop {
            digits[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            self = self.s(&[digits[n]]);
        }
        self
    }

    fn hex(mut self, v: u64) -> Self {
        self = self.s(b"0x");
        for i in (0..16).rev() {
            let nibble = ((v >> (i * 4)) & 0xf) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            self = self.s(&[c]);
        }
        self
    }

    fn print(self) {
        let mut written = 0u32;
        // SAFETY: `self.buf` is `self.n` bytes long and the handle is the
        // process's standard output.
        unsafe {
            WriteFile(
                GetStdHandle(STD_OUTPUT_HANDLE),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            );
        }
    }
}

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;

/// The checks, numbered as they run so a failure names itself and the
/// exit status is the check that failed.
struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")")
            .s(CRLF)
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// An HRESULT, printed as the unsigned 32-bit value a reader expects.
    #[track_caller]
    fn hr(&mut self, got: i32, want: i32) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }
}

fn section(what: &[u8]) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what).s(CRLF).print();
}

// winerror.h and WebServices.h's own codes, the ones this program sees.
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const WS_E_INVALID_FORMAT: i32 = 0x803d_0000u32 as i32;
const WS_E_OBJECT_FAULTED: i32 = 0x803d_0001u32 as i32;
const WS_E_INVALID_OPERATION: i32 = 0x803d_0003u32 as i32;
const WS_E_QUOTA_EXCEEDED: i32 = 0x803d_0008u32 as i32;
const WS_E_NUMERIC_OVERFLOW: i32 = 0x803d_0002u32 as i32;

/// A value no call under test returns, so a check that sees it is
/// reading an output the call did not write.
const SENTINEL: u32 = 0xffff_ffff;

/// A document with one text element and one number, the shapes
/// setup.exe's answer is made of.
const DOC: &[u8] = b"<r><a>hello</a><n>42</n></r>";

/// The same, with an end tag that closes the wrong element.
const BAD: &[u8] = b"<r><a></r>";

/// Three elements deep, to see a depth limit bite.
const DEEP: &[u8] = b"<r><b><c>1</c></b></r>";

/// Two root elements: a fragment, not a document.
const TWO_ROOTS: &[u8] = b"<a>1</a><b>2</b>";

/// An element with nothing in it, written both ways.
const EMPTY: &[u8] = b"<r><e></e></r>";
const SELF_CLOSED: &[u8] = b"<r><e/></r>";

/// A number too big for 32 bits, and text that is no number at all.
const TOO_BIG: &[u8] = b"<r><n>99999999999</n></r>";
const NOT_A_NUMBER: &[u8] = b"<r><n>abc</n></r>";

/// A root element with an attribute, a text element and a number: the
/// shape a described struct is read from.
const STRUCT_DOC: &[u8] = b"<r a=\"7\"><t>hello</t><n>42</n></r>";

/// The same, without the number element.
const NO_NUMBER: &[u8] = b"<r a=\"7\"><t>hello</t></r>";

/// The same, with an element after the last field's.
const TRAILING: &[u8] = b"<r a=\"7\"><t>hello</t><n>42</n><x>1</x></r>";

/// The same, with an attribute no field names.
const OTHER_ATTRIBUTE: &[u8] = b"<r a=\"7\" b=\"9\"><t>hello</t><n>42</n></r>";

/// The same two elements, in the other order.
const OUT_OF_ORDER: &[u8] = b"<r a=\"7\"><n>42</n><t>hello</t></r>";

/// An element whose content is an element with a number in it.
const NESTED: &[u8] = b"<r><inner><k>5</k></inner></r>";

/// Three elements of the same name, for a repeating field.
const REPEATED: &[u8] = b"<r><i>1</i><i>2</i><i>3</i></r>";

/// An element with an attribute and text of its own.
const TEXT_BODY: &[u8] = b"<r a=\"7\">hello</r>";

/// A wrapper element with no items in it.
const EMPTY_LIST: &[u8] = b"<r><list></list></r>";

/// Three elements of the same name inside a wrapper element.
const WRAPPED: &[u8] = b"<r><list><i>1</i><i>2</i><i>3</i></list></r>";

/// The struct's two elements with no attribute on the root.
const NO_ATTRIBUTE: &[u8] = b"<r><t>hello</t><n>42</n></r>";

/// The same, in a default namespace.
const NS_DOC: &[u8] = b"<r xmlns=\"urn:x\"><t>hello</t><n>42</n></r>";

/// An element whose content is an element, which is no string.
const CHILD: &[u8] = b"<r><a><b>x</b></a></r>";

/// `text`, NUL-terminated, against `want` as ASCII: the index of the
/// first differing unit and that unit, or None when the two are equal.
fn wide_diff(text: *const u16, want: &[u8]) -> Option<(usize, u64)> {
    for (i, &b) in want.iter().enumerate() {
        // SAFETY: a NUL-terminated string at least as long as the match
        // so far; the loop stops at the first unit that differs, so it
        // never reads past the terminator.
        let c = unsafe { *text.add(i) };
        if c != u16::from(b) {
            return Some((i, u64::from(c)));
        }
    }
    // SAFETY: every unit before this one matched `want`, so the string
    // is at least this long.
    let end = unsafe { *text.add(want.len()) };
    if end != 0 {
        return Some((want.len(), u64::from(end)));
    }
    None
}

/// A failing `wide_diff` as one number to report: the index above the
/// unit that differs.
fn diff_code(diff: Option<(usize, u64)>) -> u64 {
    diff.map_or(0, |(i, c)| ((i as u64) << 32) | c)
}

/// A heap and an error object, the two things every other call takes.
fn objects(c: &mut Checks) -> Result<(*mut c_void, *mut c_void), u32> {
    let mut heap: *mut c_void = null_mut();
    let mut error: *mut c_void = null_mut();

    // SAFETY: the shapes WebServices.h declares; this program owns both
    // objects and frees them at the end of `run`.
    unsafe {
        c.hr(WsCreateHeap(1 << 16, 0, null(), 0, &mut heap, null_mut()), S_OK)?;
        c.ok(!heap.is_null(), 0)?;
        // No heap to write the handle to is a bad argument, not a crash.
        c.hr(WsCreateHeap(1 << 16, 0, null(), 0, null_mut(), null_mut()), E_INVALIDARG)?;

        c.hr(WsCreateError(null(), 0, &mut error), S_OK)?;
        c.ok(!error.is_null(), 0)?;
    }
    Ok((heap, error))
}

/// A fresh error object carries no strings, and asking for one says so.
fn errors(c: &mut Checks, error: *mut c_void) -> Result<(), u32> {
    let mut count = SENTINEL;
    let mut string = WsString { length: SENTINEL, chars: null_mut() };

    // SAFETY: `count` and `string` are this frame's, and the sizes
    // passed are their own.
    unsafe {
        c.hr(
            WsGetErrorProperty(error, WS_ERROR_PROPERTY_STRING_COUNT, (&raw mut count).cast(), 4),
            S_OK,
        )?;
        c.eq(u64::from(count), 0)?;
        // A buffer too small for the property type is a bad argument.
        c.hr(
            WsGetErrorProperty(error, WS_ERROR_PROPERTY_STRING_COUNT, (&raw mut count).cast(), 2),
            E_INVALIDARG,
        )?;
        c.eq(u64::from(count), 0)?;
        // String 0 of none: a bad argument, and the WS_STRING is left as
        // it was.
        c.hr(WsGetErrorString(error, 0, &raw mut string), E_INVALIDARG)?;
        c.eq(u64::from(string.length), u64::from(SENTINEL))?;
        c.ok(string.chars.is_null(), 0)?;
    }
    Ok(())
}

/// Reading the document: to an element, into it, and its content as a
/// string and as a number -- the sequence setup.exe reads its answer
/// with.
fn reading(c: &mut Checks, heap: *mut c_void, error: *mut c_void) -> Result<(), u32> {
    let encoding = WsXmlReaderTextEncoding {
        encoding_type: WS_XML_READER_ENCODING_TYPE_TEXT,
        char_set: WS_CHARSET_UTF8,
    };
    let input = WsXmlReaderBufferInput {
        input_type: WS_XML_READER_INPUT_TYPE_BUFFER,
        encoded_data: DOC.as_ptr().cast(),
        encoded_data_size: DOC.len() as u32,
    };
    let root = WsXmlString::new(b"r");
    let text_element = WsXmlString::new(b"a");
    let number_element = WsXmlString::new(b"n");

    let mut reader: *mut c_void = null_mut();
    let mut writer: *mut c_void = null_mut();
    let mut found = SENTINEL as i32;
    let mut text: *mut u16 = null_mut();
    let mut number = SENTINEL;

    // SAFETY: the reader and writer are this function's and are freed
    // before it returns; `text` is the heap's and outlives the reads.
    unsafe {
        c.hr(WsCreateReader(null(), 0, &mut reader, error), S_OK)?;
        c.ok(!reader.is_null(), 0)?;
        c.hr(WsCreateWriter(null(), 0, &mut writer, error), S_OK)?;
        c.ok(!writer.is_null(), 0)?;

        c.hr(
            WsSetInput(
                reader,
                (&raw const encoding).cast(),
                (&raw const input).cast(),
                null(),
                0,
                error,
            ),
            S_OK,
        )?;
        // The whole document is in the buffer, so a fill of its length
        // is satisfied at once.
        c.hr(WsFillReader(reader, DOC.len() as u32, null(), error), S_OK)?;

        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;

        c.hr(WsReadToStartElement(reader, &text_element, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
        // WS_ELEMENT_TYPE_MAPPING reads the element the reader is on,
        // start tag to end tag, so the string comes back without a
        // WsReadStartElement of its own.
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_TYPE_MAPPING,
                WS_WSZ_TYPE,
                null(),
                WS_READ_REQUIRED_POINTER,
                heap,
                (&raw mut text).cast(),
                size_of::<*mut u16>() as u32,
                error,
            ),
            S_OK,
        )?;
        c.ok(!text.is_null(), 0)?;
        let diff = wide_diff(text, b"hello");
        c.ok(diff.is_none(), diff_code(diff))?;

        c.hr(WsReadToStartElement(reader, &number_element, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_TYPE_MAPPING,
                WS_UINT32_TYPE,
                null(),
                WS_READ_REQUIRED_VALUE,
                heap,
                (&raw mut number).cast(),
                4,
                error,
            ),
            S_OK,
        )?;
        c.eq(u64::from(number), 42)?;

        WsFreeWriter(writer);
        WsFreeReader(reader);
    }
    Ok(())
}

/// A document that does not close its elements in order: the read that
/// meets the mismatch says so, every later call on that reader says the
/// object is faulted, and the error object carries what happened.
fn faulted(c: &mut Checks, heap: *mut c_void, error: *mut c_void) -> Result<(), u32> {
    let encoding = WsXmlReaderTextEncoding {
        encoding_type: WS_XML_READER_ENCODING_TYPE_TEXT,
        char_set: WS_CHARSET_UTF8,
    };
    let input = WsXmlReaderBufferInput {
        input_type: WS_XML_READER_INPUT_TYPE_BUFFER,
        encoded_data: BAD.as_ptr().cast(),
        encoded_data_size: BAD.len() as u32,
    };
    let root = WsXmlString::new(b"r");
    let text_element = WsXmlString::new(b"a");

    let mut reader: *mut c_void = null_mut();
    let mut found = SENTINEL as i32;
    let mut text: *mut u16 = null_mut();
    let mut count = SENTINEL;
    let mut string = WsString { length: SENTINEL, chars: null_mut() };

    // SAFETY: as in `reading`; the reader is freed before returning.
    unsafe {
        c.hr(WsCreateReader(null(), 0, &mut reader, error), S_OK)?;
        c.hr(
            WsSetInput(
                reader,
                (&raw const encoding).cast(),
                (&raw const input).cast(),
                null(),
                0,
                error,
            ),
            S_OK,
        )?;
        c.hr(WsFillReader(reader, BAD.len() as u32, null(), error), S_OK)?;
        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        // <a> is there to be found; it is reading past it that meets
        // the end tag of r where the end tag of a belongs.
        c.hr(WsReadToStartElement(reader, &text_element, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
        c.hr(WsReadStartElement(reader, error), WS_E_INVALID_FORMAT)?;
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_TYPE_MAPPING,
                WS_WSZ_TYPE,
                null(),
                WS_READ_REQUIRED_POINTER,
                heap,
                (&raw mut text).cast(),
                size_of::<*mut u16>() as u32,
                error,
            ),
            WS_E_OBJECT_FAULTED,
        )?;

        c.hr(
            WsGetErrorProperty(error, WS_ERROR_PROPERTY_STRING_COUNT, (&raw mut count).cast(), 4),
            S_OK,
        )?;
        // Windows left three strings here, one per layer that failed.
        // How many a failure records is the library's own business and
        // the text is localized, so what is checked is that the failure
        // is described at all: a string, with characters in it.
        c.ok(count >= 1, u64::from(count))?;
        c.hr(WsGetErrorString(error, 0, &raw mut string), S_OK)?;
        c.ok(string.length > 0, u64::from(string.length))?;
        c.ok(!string.chars.is_null(), 0)?;
        // Past the last string is a bad argument, as asking a fresh
        // error object was.
        c.hr(WsGetErrorString(error, count + 4, &raw mut string), E_INVALIDARG)?;

        WsFreeReader(reader);
    }
    Ok(())
}

/// A reader over `doc`, created with the properties given and filled
/// with the whole document: what every section below starts from.
///
/// The input and encoding structures are this function's, which the
/// reader must not keep: Windows' reader copies what they say, and the
/// document's own bytes stay the caller's (`doc` outlives the reader).
fn reader_over(
    c: &mut Checks,
    doc: &[u8],
    properties: *const c_void,
    property_count: u32,
    error: *mut c_void,
) -> Result<*mut c_void, u32> {
    let encoding = WsXmlReaderTextEncoding {
        encoding_type: WS_XML_READER_ENCODING_TYPE_TEXT,
        char_set: WS_CHARSET_UTF8,
    };
    let input = WsXmlReaderBufferInput {
        input_type: WS_XML_READER_INPUT_TYPE_BUFFER,
        encoded_data: doc.as_ptr().cast(),
        encoded_data_size: doc.len() as u32,
    };
    let mut reader: *mut c_void = null_mut();
    // SAFETY: the shapes WebServices.h declares, and a handle this
    // frame owns until it is returned.
    unsafe {
        c.hr(WsCreateReader(properties, property_count, &mut reader, error), S_OK)?;
        c.ok(!reader.is_null(), 0)?;
        c.hr(
            WsSetInput(
                reader,
                (&raw const encoding).cast(),
                (&raw const input).cast(),
                null(),
                0,
                error,
            ),
            S_OK,
        )?;
        c.hr(WsFillReader(reader, doc.len() as u32, null(), error), S_OK)?;
    }
    Ok(reader)
}

/// One reader property, as the caller hands it over.
fn one(id: u32, value: *mut c_void, value_size: u32) -> [WsXmlReaderProperty; 1] {
    [WsXmlReaderProperty { id, value, value_size }]
}

/// That the reader is faulted: a property it reports for every usable
/// reader is refused instead.
fn is_faulted(c: &mut Checks, reader: *mut c_void, error: *mut c_void) -> Result<(), u32> {
    let mut row: u64 = u64::from(SENTINEL);
    // SAFETY: an eight-byte buffer, passed as the eight bytes the row is.
    unsafe {
        c.hr(
            WsGetReaderProperty(
                reader,
                WS_XML_READER_PROPERTY_ROW,
                (&raw mut row).cast(),
                8,
                error,
            ),
            WS_E_OBJECT_FAULTED,
        )?;
    }
    Ok(())
}

/// One four-byte property of the reader.
fn property(c: &mut Checks, reader: *mut c_void, id: u32, error: *mut c_void) -> Result<u32, u32> {
    let mut got: u32 = SENTINEL;
    // SAFETY: a four-byte buffer, passed as the four bytes it is.
    unsafe {
        c.hr(WsGetReaderProperty(reader, id, (&raw mut got).cast(), 4, error), S_OK)?;
    }
    Ok(got)
}

/// Where the reader stands: the row, counted from 0 (the error strings
/// count it from 1), and the column, as many bytes into the line, which
/// is the end of the node the reader is on.
fn at(
    c: &mut Checks,
    reader: *mut c_void,
    row: u64,
    column: u64,
    error: *mut c_void,
) -> Result<(), u32> {
    // SAFETY: an eight-byte buffer, passed as the eight bytes the two
    // position properties are.
    unsafe {
        let mut got: u64 = u64::from(SENTINEL);
        c.hr(
            WsGetReaderProperty(
                reader,
                WS_XML_READER_PROPERTY_ROW,
                (&raw mut got).cast(),
                8,
                error,
            ),
            S_OK,
        )?;
        c.eq(got, row)?;
        let mut got: u64 = u64::from(SENTINEL);
        c.hr(
            WsGetReaderProperty(
                reader,
                WS_XML_READER_PROPERTY_COLUMN,
                (&raw mut got).cast(),
                8,
                error,
            ),
            S_OK,
        )?;
        c.eq(got, column)?;
    }
    Ok(())
}

/// A reader's settings: which ones a reader may be created with, which
/// ones it answers for, the defaults it reports, and where its two
/// positions stand as a document is read.
fn properties(c: &mut Checks, heap: *mut c_void, error: *mut c_void) -> Result<(), u32> {
    let mut value: u32 = 1;
    let mut wide: u64 = 1;
    let v = (&raw mut value).cast::<c_void>();
    let w = (&raw mut wide).cast::<c_void>();

    // SAFETY: every property array below outlives its own call, and
    // each reader a call hands back is freed here.
    unsafe {
        // A property the reader reports and does not take (the two
        // positions, the character set, whether it is in an attribute,
        // and the id one past the last WebServices.h names), an id that
        // is no property at all, a value of the wrong size, and no
        // value: each is a bad argument and no reader.
        for properties in [
            one(WS_XML_READER_PROPERTY_CHARSET, v, 4),
            one(WS_XML_READER_PROPERTY_ROW, w, 8),
            one(WS_XML_READER_PROPERTY_COLUMN, w, 8),
            one(WS_XML_READER_PROPERTY_IN_ATTRIBUTE, v, 4),
            one(15, v, 4),
            one(99, v, 4),
            one(WS_XML_READER_PROPERTY_MAX_DEPTH, v, 2),
            one(WS_XML_READER_PROPERTY_MAX_DEPTH, null_mut(), 4),
        ] {
            let mut reader: *mut c_void = null_mut();
            c.hr(WsCreateReader(properties.as_ptr().cast(), 1, &mut reader, error), E_INVALIDARG)?;
            c.ok(reader.is_null(), reader as u64)?;
        }
        // The same property twice.
        let twice = [
            WsXmlReaderProperty { id: WS_XML_READER_PROPERTY_MAX_DEPTH, value: v, value_size: 4 },
            WsXmlReaderProperty { id: WS_XML_READER_PROPERTY_MAX_DEPTH, value: v, value_size: 4 },
        ];
        let mut reader: *mut c_void = null_mut();
        c.hr(WsCreateReader(twice.as_ptr().cast(), 2, &mut reader, error), E_INVALIDARG)?;
        c.ok(reader.is_null(), reader as u64)?;

        // The settings a reader takes, the stream and MIME ones it has
        // nothing to do with among them: all accepted.
        for id in [
            WS_XML_READER_PROPERTY_MAX_DEPTH,
            WS_XML_READER_PROPERTY_ALLOW_FRAGMENT,
            WS_XML_READER_PROPERTY_MAX_ATTRIBUTES,
            WS_XML_READER_PROPERTY_READ_DECLARATION,
            WS_XML_READER_PROPERTY_UTF8_TRIM_SIZE,
            WS_XML_READER_PROPERTY_STREAM_BUFFER_SIZE,
            10,
            11,
            12,
            WS_XML_READER_PROPERTY_ALLOW_INVALID_REFS,
            WS_XML_READER_PROPERTY_MAX_NAMESPACES,
        ] {
            let mut reader: *mut c_void = null_mut();
            c.hr(WsCreateReader(one(id, v, 4).as_ptr().cast(), 1, &mut reader, error), S_OK)?;
            c.ok(!reader.is_null(), u64::from(id))?;
            WsFreeReader(reader);
        }

        // A reader with no input answers for nothing, whatever is asked
        // of it: a property it would report, one it would not, and a
        // buffer of the wrong size are the same refusal, and the buffer
        // is left alone.
        let mut reader: *mut c_void = null_mut();
        c.hr(WsCreateReader(null(), 0, &mut reader, error), S_OK)?;
        for (id, size) in [
            (WS_XML_READER_PROPERTY_MAX_DEPTH, 4),
            (WS_XML_READER_PROPERTY_UTF8_TRIM_SIZE, 4),
            (WS_XML_READER_PROPERTY_MAX_DEPTH, 2),
        ] {
            let mut got: u32 = SENTINEL;
            c.hr(
                WsGetReaderProperty(reader, id, (&raw mut got).cast(), size, error),
                WS_E_INVALID_OPERATION,
            )?;
            c.eq(u64::from(got), u64::from(SENTINEL))?;
        }
        WsFreeReader(reader);
    }

    // The defaults, on a reader created with no properties at all.
    let reader = reader_over(c, DOC, null(), 0, error)?;
    for (id, want) in [
        (WS_XML_READER_PROPERTY_MAX_DEPTH, 32),
        (WS_XML_READER_PROPERTY_ALLOW_FRAGMENT, 0),
        (WS_XML_READER_PROPERTY_MAX_ATTRIBUTES, 128),
        (WS_XML_READER_PROPERTY_READ_DECLARATION, 1),
        (WS_XML_READER_PROPERTY_CHARSET, WS_CHARSET_UTF8),
        (WS_XML_READER_PROPERTY_IN_ATTRIBUTE, 0),
        (WS_XML_READER_PROPERTY_MAX_NAMESPACES, 32),
    ] {
        let got = property(c, reader, id, error)?;
        c.eq(u64::from(got), u64::from(want))?;
    }

    // Where the reader stands, read by read. A read that consumes a
    // node reads ahead to the one after it, which is what these
    // positions show: 18 is past <n>'s start tag, which the read of
    // <a> went on to.
    let root = WsXmlString::new(b"r");
    let text_element = WsXmlString::new(b"a");
    let number_element = WsXmlString::new(b"n");
    let mut found = SENTINEL as i32;
    let mut text: *mut u16 = null_mut();
    let mut number = SENTINEL;
    at(c, reader, 0, 0, error)?;
    // SAFETY: the reader is this function's, the names are static, and
    // `text` is the heap's.
    unsafe {
        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        at(c, reader, 0, 3, error)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        at(c, reader, 0, 6, error)?;
        c.hr(WsReadToStartElement(reader, &text_element, null(), &raw mut found, error), S_OK)?;
        at(c, reader, 0, 6, error)?;
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_TYPE_MAPPING,
                WS_WSZ_TYPE,
                null(),
                WS_READ_REQUIRED_POINTER,
                heap,
                (&raw mut text).cast(),
                size_of::<*mut u16>() as u32,
                error,
            ),
            S_OK,
        )?;
        at(c, reader, 0, 18, error)?;
        c.hr(WsReadToStartElement(reader, &number_element, null(), &raw mut found, error), S_OK)?;
        at(c, reader, 0, 18, error)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        at(c, reader, 0, 20, error)?;
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_CONTENT_TYPE_MAPPING,
                WS_UINT32_TYPE,
                null(),
                WS_READ_REQUIRED_VALUE,
                heap,
                (&raw mut number).cast(),
                4,
                error,
            ),
            S_OK,
        )?;
        c.eq(u64::from(number), 42)?;
        at(c, reader, 0, 24, error)?;
        c.hr(WsReadEndElement(reader, error), S_OK)?;
        at(c, reader, 0, 28, error)?;
        c.hr(WsReadEndElement(reader, error), S_OK)?;
        at(c, reader, 0, 28, error)?;

        // A buffer that is not the property's own size: a bad argument
        // that leaves the buffer alone and faults the reader, so every
        // call after it says the reader is faulted.
        let mut got: u32 = SENTINEL;
        c.hr(
            WsGetReaderProperty(
                reader,
                WS_XML_READER_PROPERTY_ALLOW_FRAGMENT,
                (&raw mut got).cast(),
                1,
                error,
            ),
            E_INVALIDARG,
        )?;
        c.eq(u64::from(got), u64::from(SENTINEL))?;
        c.hr(
            WsGetReaderProperty(
                reader,
                WS_XML_READER_PROPERTY_MAX_DEPTH,
                (&raw mut got).cast(),
                4,
                error,
            ),
            WS_E_OBJECT_FAULTED,
        )?;
        WsFreeReader(reader);
    }

    // A reader created with the settings it takes reports them back.
    let mut depth: u32 = 7;
    let mut yes: i32 = 1;
    let mut attributes: u32 = 5;
    let properties = [
        WsXmlReaderProperty {
            id: WS_XML_READER_PROPERTY_MAX_DEPTH,
            value: (&raw mut depth).cast(),
            value_size: 4,
        },
        WsXmlReaderProperty {
            id: WS_XML_READER_PROPERTY_ALLOW_FRAGMENT,
            value: (&raw mut yes).cast(),
            value_size: 4,
        },
        WsXmlReaderProperty {
            id: WS_XML_READER_PROPERTY_MAX_ATTRIBUTES,
            value: (&raw mut attributes).cast(),
            value_size: 4,
        },
    ];
    let reader = reader_over(c, TWO_ROOTS, properties.as_ptr().cast(), 3, error)?;
    for (id, want) in [
        (WS_XML_READER_PROPERTY_MAX_DEPTH, 7u32),
        (WS_XML_READER_PROPERTY_ALLOW_FRAGMENT, 1),
        (WS_XML_READER_PROPERTY_MAX_ATTRIBUTES, 5),
    ] {
        let got = property(c, reader, id, error)?;
        c.eq(u64::from(got), u64::from(want))?;
    }
    // SAFETY: a four-byte buffer, and the reader is done with here.
    unsafe {
        // A setting the reader takes but does not report back: the id
        // is not one it answers for.
        let mut got: u32 = SENTINEL;
        c.hr(
            WsGetReaderProperty(
                reader,
                WS_XML_READER_PROPERTY_UTF8_TRIM_SIZE,
                (&raw mut got).cast(),
                4,
                error,
            ),
            E_INVALIDARG,
        )?;
        WsFreeReader(reader);
    }
    Ok(())
}

/// WS_ELEMENT_CONTENT_TYPE_MAPPING: the value inside the element whose
/// start tag the caller has read, leaving the reader on the end tag --
/// the way setup.exe reads the configuration service's answer. The
/// element mapping reads the start tag too and leaves the reader past
/// the end tag instead, so each refuses where the other belongs.
fn content(c: &mut Checks, heap: *mut c_void, error: *mut c_void) -> Result<(), u32> {
    let root = WsXmlString::new(b"r");
    let text_element = WsXmlString::new(b"a");
    let number_element = WsXmlString::new(b"n");
    let empty_element = WsXmlString::new(b"e");

    let reader = reader_over(c, DOC, null(), 0, error)?;
    let mut found = SENTINEL as i32;
    let mut text: *mut u16 = null_mut();
    let mut number = SENTINEL;
    // SAFETY: the reader is this function's and is freed before it
    // returns; `text` is the heap's and outlives the reads.
    unsafe {
        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        c.hr(WsReadToStartElement(reader, &text_element, null(), &raw mut found, error), S_OK)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_CONTENT_TYPE_MAPPING,
                WS_WSZ_TYPE,
                null(),
                WS_READ_REQUIRED_POINTER,
                heap,
                (&raw mut text).cast(),
                size_of::<*mut u16>() as u32,
                error,
            ),
            S_OK,
        )?;
        c.ok(!text.is_null(), 0)?;
        let diff = wide_diff(text, b"hello");
        c.ok(diff.is_none(), diff_code(diff))?;
        // On the end tag of <a>: the caller's own WsReadEndElement
        // leaves the element, and the reader is then on <n>.
        c.hr(WsReadEndElement(reader, error), S_OK)?;
        c.hr(WsReadToStartElement(reader, &number_element, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_CONTENT_TYPE_MAPPING,
                WS_UINT32_TYPE,
                null(),
                WS_READ_REQUIRED_VALUE,
                heap,
                (&raw mut number).cast(),
                4,
                error,
            ),
            S_OK,
        )?;
        c.eq(u64::from(number), 42)?;
        // Out of <n>, then out of <r>: the end of the document, where
        // there is no start element to find and no end tag to read.
        c.hr(WsReadEndElement(reader, error), S_OK)?;
        c.hr(WsReadEndElement(reader, error), S_OK)?;
        found = SENTINEL as i32;
        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 0)?;
        c.hr(WsReadEndElement(reader, error), WS_E_INVALID_FORMAT)?;
        WsFreeReader(reader);
    }

    // The content mapping where the element mapping belongs, on a
    // start element: not refused, because a start tag is not text --
    // there is no content, so a string is the empty one and the reader
    // has not moved. It then reads into the element as it would have.
    let reader = reader_over(c, DOC, null(), 0, error)?;
    // SAFETY: as above.
    unsafe {
        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        c.hr(WsReadToStartElement(reader, &text_element, null(), &raw mut found, error), S_OK)?;
        at(c, reader, 0, 6, error)?;
        text = null_mut();
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_CONTENT_TYPE_MAPPING,
                WS_WSZ_TYPE,
                null(),
                WS_READ_REQUIRED_POINTER,
                heap,
                (&raw mut text).cast(),
                size_of::<*mut u16>() as u32,
                error,
            ),
            S_OK,
        )?;
        let diff = wide_diff(text, b"");
        c.ok(diff.is_none(), diff_code(diff))?;
        at(c, reader, 0, 6, error)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        at(c, reader, 0, 11, error)?;
        WsFreeReader(reader);
    }

    // A number has no empty form, so the same position is where a
    // number is asked for and nothing is found: the text is not a
    // number, and the reader is faulted after it.
    let reader = reader_over(c, DOC, null(), 0, error)?;
    // SAFETY: as above.
    unsafe {
        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        c.hr(WsReadToStartElement(reader, &number_element, null(), &raw mut found, error), S_OK)?;
        number = SENTINEL;
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_CONTENT_TYPE_MAPPING,
                WS_UINT32_TYPE,
                null(),
                WS_READ_REQUIRED_VALUE,
                heap,
                (&raw mut number).cast(),
                4,
                error,
            ),
            WS_E_INVALID_FORMAT,
        )?;
        c.eq(u64::from(number), u64::from(SENTINEL))?;
        is_faulted(c, reader, error)?;
        WsFreeReader(reader);
    }

    // The element mapping where the content mapping belongs, inside an
    // element: it wants a start element and the text is not one.
    let reader = reader_over(c, DOC, null(), 0, error)?;
    // SAFETY: as above.
    unsafe {
        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        c.hr(WsReadToStartElement(reader, &text_element, null(), &raw mut found, error), S_OK)?;
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        text = null_mut();
        c.hr(
            WsReadType(
                reader,
                WS_ELEMENT_TYPE_MAPPING,
                WS_WSZ_TYPE,
                null(),
                WS_READ_REQUIRED_POINTER,
                heap,
                (&raw mut text).cast(),
                size_of::<*mut u16>() as u32,
                error,
            ),
            WS_E_INVALID_FORMAT,
        )?;
        c.ok(text.is_null(), 1)?;
        is_faulted(c, reader, error)?;
        WsFreeReader(reader);
    }

    // A number the document does not hold: too big for the type, and
    // not a number at all. Neither touches the caller's value, and both
    // fault the reader -- every read of a value the document does not
    // hold does.
    for (doc, want) in [(TOO_BIG, WS_E_NUMERIC_OVERFLOW), (NOT_A_NUMBER, WS_E_INVALID_FORMAT)] {
        let reader = reader_over(c, doc, null(), 0, error)?;
        // SAFETY: as above.
        unsafe {
            c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
            c.hr(WsReadStartElement(reader, error), S_OK)?;
            c.hr(
                WsReadToStartElement(reader, &number_element, null(), &raw mut found, error),
                S_OK,
            )?;
            c.hr(WsReadStartElement(reader, error), S_OK)?;
            number = SENTINEL;
            c.hr(
                WsReadType(
                    reader,
                    WS_ELEMENT_CONTENT_TYPE_MAPPING,
                    WS_UINT32_TYPE,
                    null(),
                    WS_READ_REQUIRED_VALUE,
                    heap,
                    (&raw mut number).cast(),
                    4,
                    error,
                ),
                want,
            )?;
            c.eq(u64::from(number), u64::from(SENTINEL))?;
            is_faulted(c, reader, error)?;
            WsFreeReader(reader);
        }
    }

    // An element whose content is an element: no string, read whole;
    // the empty string, read as content, with the child's start tag
    // left for the caller -- and the reader is not faulted by it.
    for content_mapping in [false, true] {
        let reader = reader_over(c, CHILD, null(), 0, error)?;
        // SAFETY: as above.
        unsafe {
            c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
            c.hr(WsReadStartElement(reader, error), S_OK)?;
            c.hr(WsReadToStartElement(reader, &text_element, null(), &raw mut found, error), S_OK)?;
            if content_mapping {
                c.hr(WsReadStartElement(reader, error), S_OK)?;
            }
            text = null_mut();
            let hr = WsReadType(
                reader,
                if content_mapping {
                    WS_ELEMENT_CONTENT_TYPE_MAPPING
                } else {
                    WS_ELEMENT_TYPE_MAPPING
                },
                WS_WSZ_TYPE,
                null(),
                WS_READ_REQUIRED_POINTER,
                heap,
                (&raw mut text).cast(),
                size_of::<*mut u16>() as u32,
                error,
            );
            if content_mapping {
                c.hr(hr, S_OK)?;
                let diff = wide_diff(text, b"");
                c.ok(diff.is_none(), diff_code(diff))?;
                let child = WsXmlString::new(b"b");
                found = SENTINEL as i32;
                c.hr(WsReadToStartElement(reader, &child, null(), &raw mut found, error), S_OK)?;
                c.eq(found as u32 as u64, 1)?;
            } else {
                c.hr(hr, WS_E_INVALID_FORMAT)?;
                // What the caller's buffer holds after a read that
                // failed is no part of the contract and nothing a
                // caller may read: Windows puts an empty string there
                // for this failure and nothing for the one above.
                is_faulted(c, reader, error)?;
            }
            WsFreeReader(reader);
        }
    }

    // An element with nothing in it, written both ways: its content is
    // the empty string, and the reader is on its end tag.
    for doc in [EMPTY, SELF_CLOSED] {
        let reader = reader_over(c, doc, null(), 0, error)?;
        // SAFETY: as above.
        unsafe {
            c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
            c.hr(WsReadStartElement(reader, error), S_OK)?;
            c.hr(
                WsReadToStartElement(reader, &empty_element, null(), &raw mut found, error),
                S_OK,
            )?;
            c.eq(found as u32 as u64, 1)?;
            c.hr(WsReadStartElement(reader, error), S_OK)?;
            text = null_mut();
            c.hr(
                WsReadType(
                    reader,
                    WS_ELEMENT_CONTENT_TYPE_MAPPING,
                    WS_WSZ_TYPE,
                    null(),
                    WS_READ_REQUIRED_POINTER,
                    heap,
                    (&raw mut text).cast(),
                    size_of::<*mut u16>() as u32,
                    error,
                ),
                S_OK,
            )?;
            c.ok(!text.is_null(), 0)?;
            let diff = wide_diff(text, b"");
            c.ok(diff.is_none(), diff_code(diff))?;
            c.hr(WsReadEndElement(reader, error), S_OK)?;
            WsFreeReader(reader);
        }
    }
    Ok(())
}

/// WS_XML_READER_PROPERTY_ALLOW_FRAGMENT: more than one root element,
/// which is what setup.exe reads the answer as. Without it, the end tag
/// of the first root is where the reader meets content after the root
/// element -- it reads ahead, so the failure is there and not at the
/// next read -- and the reader is faulted after.
fn fragments(c: &mut Checks, heap: *mut c_void, error: *mut c_void) -> Result<(), u32> {
    let first = WsXmlString::new(b"a");
    let second = WsXmlString::new(b"b");
    let third = WsXmlString::new(b"c");

    for allow in [false, true] {
        let mut yes: i32 = 1;
        let properties = one(WS_XML_READER_PROPERTY_ALLOW_FRAGMENT, (&raw mut yes).cast(), 4);
        let (p, n) = if allow { (properties.as_ptr().cast(), 1) } else { (null::<c_void>(), 0u32) };
        let reader = reader_over(c, TWO_ROOTS, p, n, error)?;
        let mut found = SENTINEL as i32;
        let mut number = SENTINEL;
        // SAFETY: the property array outlives the call; the reader is
        // freed at the end of the pass.
        unsafe {
            c.hr(WsReadToStartElement(reader, &first, null(), &raw mut found, error), S_OK)?;
            c.eq(found as u32 as u64, 1)?;
            c.hr(WsReadStartElement(reader, error), S_OK)?;
            c.hr(
                WsReadType(
                    reader,
                    WS_ELEMENT_CONTENT_TYPE_MAPPING,
                    WS_UINT32_TYPE,
                    null(),
                    WS_READ_REQUIRED_VALUE,
                    heap,
                    (&raw mut number).cast(),
                    4,
                    error,
                ),
                S_OK,
            )?;
            c.eq(u64::from(number), 1)?;
            if !allow {
                c.hr(WsReadEndElement(reader, error), WS_E_INVALID_FORMAT)?;
                c.hr(
                    WsReadToStartElement(reader, &second, null(), &raw mut found, error),
                    WS_E_OBJECT_FAULTED,
                )?;
                WsFreeReader(reader);
                continue;
            }
            c.hr(WsReadEndElement(reader, error), S_OK)?;
            found = SENTINEL as i32;
            c.hr(WsReadToStartElement(reader, &second, null(), &raw mut found, error), S_OK)?;
            c.eq(found as u32 as u64, 1)?;
            c.hr(WsReadStartElement(reader, error), S_OK)?;
            number = SENTINEL;
            c.hr(
                WsReadType(
                    reader,
                    WS_ELEMENT_CONTENT_TYPE_MAPPING,
                    WS_UINT32_TYPE,
                    null(),
                    WS_READ_REQUIRED_VALUE,
                    heap,
                    (&raw mut number).cast(),
                    4,
                    error,
                ),
                S_OK,
            )?;
            c.eq(u64::from(number), 2)?;
            c.hr(WsReadEndElement(reader, error), S_OK)?;
            // Past the last root: no start element to find.
            found = SENTINEL as i32;
            c.hr(WsReadToStartElement(reader, &third, null(), &raw mut found, error), S_OK)?;
            c.eq(found as u32 as u64, 0)?;
            WsFreeReader(reader);
        }
    }

    // A fragment read root by root with the element mapping, which
    // takes the whole of one root at a time.
    let mut yes: i32 = 1;
    let properties = one(WS_XML_READER_PROPERTY_ALLOW_FRAGMENT, (&raw mut yes).cast(), 4);
    let reader = reader_over(c, TWO_ROOTS, properties.as_ptr().cast(), 1, error)?;
    let mut found = SENTINEL as i32;
    let mut number = SENTINEL;
    // SAFETY: as above.
    unsafe {
        for (name, want) in [(&first, 1u32), (&second, 2)] {
            c.hr(WsReadToStartElement(reader, name, null(), &raw mut found, error), S_OK)?;
            c.eq(found as u32 as u64, 1)?;
            c.hr(
                WsReadType(
                    reader,
                    WS_ELEMENT_TYPE_MAPPING,
                    WS_UINT32_TYPE,
                    null(),
                    WS_READ_REQUIRED_VALUE,
                    heap,
                    (&raw mut number).cast(),
                    4,
                    error,
                ),
                S_OK,
            )?;
            c.eq(u64::from(number), u64::from(want))?;
        }
        WsFreeReader(reader);
    }
    Ok(())
}

/// WS_XML_READER_PROPERTY_MAX_DEPTH: how deep a start element may be.
/// The read that reaches a deeper tag is WS_E_QUOTA_EXCEEDED and faults
/// the reader -- with a limit of 2 that is the read which moves into
/// <b>, because reading ahead is what reaches <c> -- and a limit of 3
/// lets the whole of <r><b><c> through.
fn depth(c: &mut Checks, error: *mut c_void) -> Result<(), u32> {
    for (limit, stops) in [(2u32, true), (3, false)] {
        let mut value = limit;
        let properties = one(WS_XML_READER_PROPERTY_MAX_DEPTH, (&raw mut value).cast(), 4);
        let reader = reader_over(c, DEEP, properties.as_ptr().cast(), 1, error)?;
        let mut found = SENTINEL as i32;
        // SAFETY: the property array outlives the call; the reader is
        // freed at the end of the pass.
        unsafe {
            for (i, name) in [b"r".as_slice(), b"b", b"c"].into_iter().enumerate() {
                let want = WsXmlString::new(name);
                c.hr(WsReadToStartElement(reader, &want, null(), &raw mut found, error), S_OK)?;
                c.eq(found as u32 as u64, 1)?;
                if stops && i == 1 {
                    c.hr(WsReadStartElement(reader, error), WS_E_QUOTA_EXCEEDED)?;
                    let third = WsXmlString::new(b"c");
                    c.hr(
                        WsReadToStartElement(reader, &third, null(), &raw mut found, error),
                        WS_E_OBJECT_FAULTED,
                    )?;
                    break;
                }
                c.hr(WsReadStartElement(reader, error), S_OK)?;
            }
            WsFreeReader(reader);
        }
    }
    Ok(())
}

/// WebServices.h's WS_FIELD_DESCRIPTION: how one field of a described
/// struct is found in the document and where its value goes.
#[repr(C)]
struct WsFieldDescription {
    mapping: u32,
    local_name: *const WsXmlString,
    ns: *const WsXmlString,
    ws_type: u32,
    type_description: *const c_void,
    offset: u32,
    options: u32,
    default_value: *const c_void,
    count_offset: u32,
    item_local_name: *const WsXmlString,
    item_ns: *const WsXmlString,
    item_range: *const c_void,
}

/// WebServices.h's WS_STRUCT_DESCRIPTION: the struct a WS_STRUCT_TYPE
/// read fills, its fields and its XML type name.
#[repr(C)]
struct WsStructDescription {
    size: u32,
    alignment: u32,
    fields: *const *const WsFieldDescription,
    field_count: u32,
    type_local_name: *const WsXmlString,
    type_ns: *const WsXmlString,
    parent_type: *const c_void,
    sub_types: *const c_void,
    sub_type_count: u32,
    struct_options: u32,
}

/// One field, with what no case here uses left NULL.
fn field(
    mapping: u32,
    local_name: *const WsXmlString,
    ws_type: u32,
    type_description: *const c_void,
    offset: usize,
    options: u32,
) -> WsFieldDescription {
    WsFieldDescription {
        mapping,
        local_name,
        ns: &NO_NS,
        ws_type,
        type_description,
        offset: offset as u32,
        options,
        default_value: null(),
        count_offset: 0,
        item_local_name: null(),
        item_ns: null(),
        item_range: null(),
    }
}

/// A description of `T`, with its size and alignment as the compiler
/// lays it out, which is what a caller of WsReadType passes.
fn described<T>(
    fields: *const *const WsFieldDescription,
    field_count: u32,
    struct_options: u32,
) -> WsStructDescription {
    WsStructDescription {
        size: size_of::<T>() as u32,
        alignment: align_of::<T>() as u32,
        fields,
        field_count,
        type_local_name: &ANSWER_NAME,
        type_ns: &NO_NS,
        parent_type: null(),
        sub_types: null(),
        sub_type_count: 0,
        struct_options,
    }
}

/// The empty namespace a document with no namespaces is read against.
static NO_NS: WsXmlString = WsXmlString::new(b"");
/// The XML type name every description here carries.
static ANSWER_NAME: WsXmlString = WsXmlString::new(b"Answer");

// SAFETY: the two statics above are read-only and their `bytes` point
// into this image's constants, so sharing them between threads is
// sound; WS_XML_STRING has no interior mutability of its own.
unsafe impl Sync for WsXmlString {}

/// The struct the simple cases fill: an element string, an element
/// number and an attribute number.
#[repr(C)]
struct Answer {
    text: *mut u16,
    number: u32,
    attr: u32,
}

/// A struct whose fields are pointers to their values.
#[repr(C)]
struct Pointers {
    text: *mut u16,
    number: *mut u32,
}

/// A repeating element field: the items on the heap and their count.
#[repr(C)]
struct Items {
    items: *mut u32,
    count: u32,
}

/// A struct whose only field is another struct.
#[repr(C)]
struct Inner {
    k: u32,
}

#[repr(C)]
struct Outer {
    inner: Inner,
}

/// A reader over `doc`, positioned on its root element `<r>`.
fn at_root(c: &mut Checks, doc: &[u8], error: *mut c_void) -> Result<*mut c_void, u32> {
    let root = WsXmlString::new(b"r");
    let mut found = SENTINEL as i32;
    let reader = reader_over(c, doc, null(), 0, error)?;
    // SAFETY: a reader this frame owns, and a name that outlives the call.
    unsafe {
        c.hr(WsReadToStartElement(reader, &root, null(), &raw mut found, error), S_OK)?;
        c.eq(found as u32 as u64, 1)?;
    }
    Ok(reader)
}

/// One WS_STRUCT_TYPE read of `<r>` in `doc` against `description`,
/// into `value`: the HRESULT, with the reader freed either way.
#[allow(clippy::too_many_arguments)]
fn read_struct(
    c: &mut Checks,
    doc: &[u8],
    description: *const WsStructDescription,
    read_option: u32,
    value: *mut c_void,
    value_size: u32,
    heap: *mut c_void,
    error: *mut c_void,
) -> Result<i32, u32> {
    let reader = at_root(c, doc, error)?;
    // SAFETY: the reader is this frame's, `value` is `value_size` bytes
    // of the caller's, and the description outlives the call.
    let hr = unsafe {
        let hr = WsReadType(
            reader,
            WS_ELEMENT_TYPE_MAPPING,
            WS_STRUCT_TYPE,
            description.cast(),
            read_option,
            heap,
            value,
            value_size,
            error,
        );
        WsFreeReader(reader);
        hr
    };
    Ok(hr)
}

/// WS_STRUCT_TYPE: an element read whole into a C struct the caller
/// describes, which is what Word asks webservices.dll for and gala
/// refuses (ROADMAP OF). Measured here before any of it is written.
fn structs(c: &mut Checks, heap: *mut c_void, error: *mut c_void) -> Result<(), u32> {
    let t = WsXmlString::new(b"t");
    let n = WsXmlString::new(b"n");
    let a = WsXmlString::new(b"a");
    let i = WsXmlString::new(b"i");
    let inner = WsXmlString::new(b"inner");
    let k = WsXmlString::new(b"k");

    // The three fields of `Answer`, attributes before elements as a
    // description lists them.
    let f_attr = field(
        WS_ATTRIBUTE_FIELD_MAPPING,
        &a,
        WS_UINT32_TYPE,
        null(),
        core::mem::offset_of!(Answer, attr),
        0,
    );
    let f_text = field(
        WS_ELEMENT_FIELD_MAPPING,
        &t,
        WS_WSZ_TYPE,
        null(),
        core::mem::offset_of!(Answer, text),
        0,
    );
    let f_number = field(
        WS_ELEMENT_FIELD_MAPPING,
        &n,
        WS_UINT32_TYPE,
        null(),
        core::mem::offset_of!(Answer, number),
        0,
    );
    let fields: [*const WsFieldDescription; 3] = [&f_attr, &f_text, &f_number];
    let answer = described::<Answer>(fields.as_ptr(), 3, 0);

    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        STRUCT_DOC,
        &answer,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, S_OK)?;
    c.eq(u64::from(value.number), 42)?;
    c.eq(u64::from(value.attr), 7)?;
    c.ok(!value.text.is_null(), value.text as u64)?;
    c.eq(diff_code(wide_diff(value.text, b"hello")), 0)?;

    // A size that is not the struct's.
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        STRUCT_DOC,
        &answer,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32 - 4,
        heap,
        error,
    )?;
    c.hr(hr, E_INVALIDARG)?;

    // No description at all for a type that is one.
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        STRUCT_DOC,
        null(),
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, E_INVALIDARG)?;

    // The same read as a pointer: the struct goes on the heap.
    let mut pointer: *mut Answer = null_mut();
    let hr = read_struct(
        c,
        STRUCT_DOC,
        &answer,
        WS_READ_REQUIRED_POINTER,
        (&raw mut pointer).cast(),
        size_of::<*mut Answer>() as u32,
        heap,
        error,
    )?;
    // SAFETY: read only when the call succeeded and wrote a pointer,
    // which it then owns on the heap passed in.
    let (number, attr) = if hr == S_OK && !pointer.is_null() {
        unsafe { (u64::from((*pointer).number), u64::from((*pointer).attr)) }
    } else {
        (0, 0)
    };
    c.hr(hr, S_OK)?;
    c.eq(number, 42)?;
    c.eq(attr, 7)?;

    // A required element the document does not have.
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        NO_NUMBER,
        &answer,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, WS_E_INVALID_FORMAT)?;
    c.eq(u64::from(value.number), u64::from(SENTINEL))?;

    // The same document, with that element optional.
    let f_optional = field(
        WS_ELEMENT_FIELD_MAPPING,
        &n,
        WS_UINT32_TYPE,
        null(),
        core::mem::offset_of!(Answer, number),
        WS_FIELD_OPTIONAL,
    );
    let optional_fields: [*const WsFieldDescription; 3] = [&f_attr, &f_text, &f_optional];
    let optional = described::<Answer>(optional_fields.as_ptr(), 3, 0);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        NO_NUMBER,
        &optional,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, S_OK)?;
    c.eq(u64::from(value.number), 0)?;
    c.eq(u64::from(value.attr), 7)?;

    // An element after the last field, and the option that ignores it.
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        TRAILING,
        &answer,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, WS_E_INVALID_FORMAT)?;
    c.eq(u64::from(value.number), 42)?;
    let ignoring =
        described::<Answer>(fields.as_ptr(), 3, WS_STRUCT_IGNORE_TRAILING_ELEMENT_CONTENT);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        TRAILING,
        &ignoring,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, S_OK)?;
    c.eq(u64::from(value.number), 42)?;

    // An attribute no field names, and the option that ignores it.
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        OTHER_ATTRIBUTE,
        &answer,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, WS_E_INVALID_FORMAT)?;
    c.eq(u64::from(value.attr), 7)?;
    let lenient = described::<Answer>(fields.as_ptr(), 3, WS_STRUCT_IGNORE_UNHANDLED_ATTRIBUTES);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        OTHER_ATTRIBUTE,
        &lenient,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, S_OK)?;
    c.eq(u64::from(value.attr), 7)?;

    // The elements in the other order.
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        OUT_OF_ORDER,
        &answer,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, WS_E_INVALID_FORMAT)?;
    c.eq(u64::from(value.number), u64::from(SENTINEL))?;
    c.eq(u64::from(value.attr), 7)?;

    // A field whose namespace is NULL rather than the empty string.
    let mut f_null_ns = field(
        WS_ELEMENT_FIELD_MAPPING,
        &t,
        WS_WSZ_TYPE,
        null(),
        core::mem::offset_of!(Answer, text),
        0,
    );
    f_null_ns.ns = null();
    let null_ns_fields: [*const WsFieldDescription; 3] = [&f_attr, &f_null_ns, &f_number];
    let null_ns = described::<Answer>(null_ns_fields.as_ptr(), 3, 0);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        STRUCT_DOC,
        &null_ns,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, E_INVALIDARG)?;
    c.eq(u64::from(value.number), u64::from(SENTINEL))?;

    // A field that is itself a described struct.
    let f_k = field(
        WS_ELEMENT_FIELD_MAPPING,
        &k,
        WS_UINT32_TYPE,
        null(),
        core::mem::offset_of!(Inner, k),
        0,
    );
    let inner_fields: [*const WsFieldDescription; 1] = [&f_k];
    let inner_description = described::<Inner>(inner_fields.as_ptr(), 1, 0);
    let f_inner = field(
        WS_ELEMENT_FIELD_MAPPING,
        &inner,
        WS_STRUCT_TYPE,
        (&raw const inner_description).cast(),
        core::mem::offset_of!(Outer, inner),
        0,
    );
    let outer_fields: [*const WsFieldDescription; 1] = [&f_inner];
    let outer = described::<Outer>(outer_fields.as_ptr(), 1, 0);
    let mut value = Outer { inner: Inner { k: SENTINEL } };
    let hr = read_struct(
        c,
        NESTED,
        &outer,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Outer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, S_OK)?;
    c.eq(u64::from(value.inner.k), 5)?;

    // A repeating element, four ways: the items named by the item name
    // with no wrapper, the same with the field named too, a wrapper
    // element around them, and the items as pointers.
    let list = WsXmlString::new(b"list");
    for (wrapper, options, doc, want, count, items) in [
        // No wrapper element at all: a repeating field has to name one,
        // and the items' own name is not it.
        (null::<WsXmlString>(), 0, REPEATED, E_INVALIDARG, SENTINEL, 0),
        // The items' name as the wrapper's: the first item's text is
        // then inside the wrapper and is no item of the list, which is
        // refused -- with the count the reader got to, zero, written.
        (&raw const i, 0, REPEATED, WS_E_INVALID_FORMAT, 0, 0),
        // The wrapper the document has, which is the shape that reads.
        (&raw const list, 0, WRAPPED, S_OK, 3, 0x03_0201),
        // Items that are pointers to their values, with no wrapper
        // named either: the wrapper is answered for first.
        (null::<WsXmlString>(), WS_FIELD_POINTER, REPEATED, E_INVALIDARG, SENTINEL, 0),
    ] {
        let mut f_items = field(
            WS_REPEATING_ELEMENT_FIELD_MAPPING,
            wrapper,
            WS_UINT32_TYPE,
            null(),
            core::mem::offset_of!(Items, items),
            options,
        );
        f_items.count_offset = core::mem::offset_of!(Items, count) as u32;
        f_items.item_local_name = &i;
        f_items.item_ns = &NO_NS;
        let item_fields: [*const WsFieldDescription; 1] = [&f_items];
        let items_description = described::<Items>(item_fields.as_ptr(), 1, 0);
        let mut value = Items { items: null_mut(), count: SENTINEL };
        let hr = read_struct(
            c,
            doc,
            &items_description,
            WS_READ_REQUIRED_VALUE,
            (&raw mut value).cast(),
            size_of::<Items>() as u32,
            heap,
            error,
        )?;
        // SAFETY: read only when the call wrote three items on the heap.
        let read = if hr == S_OK && value.count == 3 && !value.items.is_null() {
            unsafe {
                u64::from(*value.items)
                    | u64::from(*value.items.add(1)) << 8
                    | u64::from(*value.items.add(2)) << 16
            }
        } else {
            0
        };
        c.hr(hr, want)?;
        c.eq(u64::from(value.count), u64::from(count))?;
        c.eq(read, items)?;
    }

    // The element's own text as a field.
    let mut f_body = field(
        WS_TEXT_FIELD_MAPPING,
        null(),
        WS_WSZ_TYPE,
        null(),
        core::mem::offset_of!(Answer, text),
        0,
    );
    f_body.ns = null();
    let text_fields: [*const WsFieldDescription; 2] = [&f_attr, &f_body];
    let texty = described::<Answer>(text_fields.as_ptr(), 2, 0);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        TEXT_BODY,
        &texty,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    let diff = if hr == S_OK && !value.text.is_null() {
        diff_code(wide_diff(value.text, b"hello"))
    } else {
        u64::from(SENTINEL)
    };
    c.hr(hr, S_OK)?;
    c.eq(u64::from(value.attr), 7)?;
    c.eq(diff, 0)?;

    // The same struct read from inside its element: the caller has read
    // the start tag, and the reader is left on the end tag.
    let reader = at_root(c, NO_ATTRIBUTE, error)?;
    let content_fields: [*const WsFieldDescription; 2] = [&f_text, &f_number];
    let content_description = described::<Answer>(content_fields.as_ptr(), 2, 0);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    // SAFETY: the reader is this frame's and the value is its own size.
    let (hr, end) = unsafe {
        c.hr(WsReadStartElement(reader, error), S_OK)?;
        let hr = WsReadType(
            reader,
            WS_ELEMENT_CONTENT_TYPE_MAPPING,
            WS_STRUCT_TYPE,
            (&raw const content_description).cast(),
            WS_READ_REQUIRED_VALUE,
            heap,
            (&raw mut value).cast(),
            size_of::<Answer>() as u32,
            error,
        );
        let end = WsReadEndElement(reader, error);
        WsFreeReader(reader);
        (hr, end)
    };
    c.hr(hr, S_OK)?;
    c.eq(u64::from(value.number), 42)?;
    c.hr(end, S_OK)?;

    // Where the reader stands after a struct read: past the element's
    // end tag, which the column says.
    let reader = at_root(c, NO_ATTRIBUTE, error)?;
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    // SAFETY: as above; the column is the eight bytes its property is.
    let (hr, column) = unsafe {
        let hr = WsReadType(
            reader,
            WS_ELEMENT_TYPE_MAPPING,
            WS_STRUCT_TYPE,
            (&raw const content_description).cast(),
            WS_READ_REQUIRED_VALUE,
            heap,
            (&raw mut value).cast(),
            size_of::<Answer>() as u32,
            error,
        );
        let mut column: u64 = u64::from(SENTINEL);
        WsGetReaderProperty(
            reader,
            WS_XML_READER_PROPERTY_COLUMN,
            (&raw mut column).cast(),
            8,
            error,
        );
        WsFreeReader(reader);
        (hr, column)
    };
    c.hr(hr, S_OK)?;
    c.eq(column, NO_ATTRIBUTE.len() as u64)?;

    // A namespace the fields have to match: the document's default one.
    let urn = WsXmlString::new(b"urn:x");
    let mut f_ns_text = field(
        WS_ELEMENT_FIELD_MAPPING,
        &t,
        WS_WSZ_TYPE,
        null(),
        core::mem::offset_of!(Answer, text),
        0,
    );
    f_ns_text.ns = &urn;
    let mut f_ns_number = field(
        WS_ELEMENT_FIELD_MAPPING,
        &n,
        WS_UINT32_TYPE,
        null(),
        core::mem::offset_of!(Answer, number),
        0,
    );
    f_ns_number.ns = &urn;
    let ns_fields: [*const WsFieldDescription; 2] = [&f_ns_text, &f_ns_number];
    let in_namespace = described::<Answer>(ns_fields.as_ptr(), 2, 0);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        NS_DOC,
        &in_namespace,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, S_OK)?;
    c.eq(u64::from(value.number), 42)?;

    // The same document against fields with no namespace.
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        NS_DOC,
        &content_description,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, WS_E_INVALID_FORMAT)?;
    c.eq(u64::from(value.number), u64::from(SENTINEL))?;

    // An attribute field the document has no attribute for.
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        NO_ATTRIBUTE,
        &answer,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, WS_E_INVALID_FORMAT)?;
    c.eq(u64::from(value.attr), u64::from(SENTINEL))?;

    // A scalar field that is a pointer to its value on the heap.
    let f_pointer_number = field(
        WS_ELEMENT_FIELD_MAPPING,
        &n,
        WS_UINT32_TYPE,
        null(),
        core::mem::offset_of!(Pointers, number),
        WS_FIELD_POINTER,
    );
    let f_pointer_text = field(
        WS_ELEMENT_FIELD_MAPPING,
        &t,
        WS_WSZ_TYPE,
        null(),
        core::mem::offset_of!(Pointers, text),
        0,
    );
    let pointer_fields: [*const WsFieldDescription; 2] = [&f_pointer_text, &f_pointer_number];
    let pointer_description = described::<Pointers>(pointer_fields.as_ptr(), 2, 0);
    let mut value = Pointers { text: null_mut(), number: null_mut() };
    let hr = read_struct(
        c,
        NO_ATTRIBUTE,
        &pointer_description,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Pointers>() as u32,
        heap,
        error,
    )?;
    // SAFETY: read only when the call wrote a pointer to the number it
    // read, which it owns on the heap passed in.
    let number = if hr == S_OK && !value.number.is_null() {
        unsafe { u64::from(*value.number) }
    } else {
        u64::from(SENTINEL)
    };
    c.hr(hr, E_INVALIDARG)?;
    c.eq(number, u64::from(SENTINEL))?;

    // A field whose offset is past the end of the struct it describes.
    let f_past_end =
        field(WS_ELEMENT_FIELD_MAPPING, &n, WS_UINT32_TYPE, null(), size_of::<Answer>(), 0);
    let past_fields: [*const WsFieldDescription; 2] = [&f_text, &f_past_end];
    let past_end = described::<Answer>(past_fields.as_ptr(), 2, 0);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        NO_ATTRIBUTE,
        &past_end,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, E_INVALIDARG)?;

    // Two fields at the same offset.
    let f_same = field(
        WS_ELEMENT_FIELD_MAPPING,
        &n,
        WS_UINT32_TYPE,
        null(),
        core::mem::offset_of!(Answer, text),
        0,
    );
    let same_fields: [*const WsFieldDescription; 2] = [&f_text, &f_same];
    let overlapping = described::<Answer>(same_fields.as_ptr(), 2, 0);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        NO_ATTRIBUTE,
        &overlapping,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, S_OK)?;

    // An element field before the attribute field in the description.
    let reversed_fields: [*const WsFieldDescription; 3] = [&f_text, &f_number, &f_attr];
    let reversed = described::<Answer>(reversed_fields.as_ptr(), 3, 0);
    let mut value = Answer { text: null_mut(), number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        STRUCT_DOC,
        &reversed,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, WS_E_INVALID_FORMAT)?;
    c.eq(u64::from(value.number), u64::from(SENTINEL))?;
    c.eq(u64::from(value.attr), u64::from(SENTINEL))?;

    // A wrapper with no items in it, and items that are pointers.
    let mut f_empty = field(
        WS_REPEATING_ELEMENT_FIELD_MAPPING,
        &list,
        WS_UINT32_TYPE,
        null(),
        core::mem::offset_of!(Items, items),
        0,
    );
    f_empty.count_offset = core::mem::offset_of!(Items, count) as u32;
    f_empty.item_local_name = &i;
    f_empty.item_ns = &NO_NS;
    let empty_fields: [*const WsFieldDescription; 1] = [&f_empty];
    let empty_list = described::<Items>(empty_fields.as_ptr(), 1, 0);
    let mut value = Items { items: usize::MAX as *mut u32, count: SENTINEL };
    let hr = read_struct(
        c,
        EMPTY_LIST,
        &empty_list,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Items>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, S_OK)?;
    c.eq(u64::from(value.count), 0)?;
    c.ok(!value.items.is_null(), value.items as u64)?;

    let mut f_item_pointers = field(
        WS_REPEATING_ELEMENT_FIELD_MAPPING,
        &list,
        WS_UINT32_TYPE,
        null(),
        core::mem::offset_of!(Items, items),
        WS_FIELD_POINTER,
    );
    f_item_pointers.count_offset = core::mem::offset_of!(Items, count) as u32;
    f_item_pointers.item_local_name = &i;
    f_item_pointers.item_ns = &NO_NS;
    let item_pointer_fields: [*const WsFieldDescription; 1] = [&f_item_pointers];
    let item_pointers = described::<Items>(item_pointer_fields.as_ptr(), 1, 0);
    let mut value = Items { items: null_mut(), count: SENTINEL };
    let hr = read_struct(
        c,
        WRAPPED,
        &item_pointers,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Items>() as u32,
        heap,
        error,
    )?;
    // SAFETY: read only when the call wrote three item pointers.
    let first = if hr == S_OK && value.count == 3 && !value.items.is_null() {
        unsafe {
            let items = value.items.cast::<*mut u32>();
            if (*items).is_null() { u64::from(SENTINEL) } else { u64::from(**items) }
        }
    } else {
        u64::from(SENTINEL)
    };
    c.hr(hr, WS_E_INVALID_FORMAT)?;
    c.eq(u64::from(value.count), u64::from(SENTINEL))?;
    c.eq(first, u64::from(SENTINEL))?;

    // An optional string field the document has no element for.
    let f_optional_text = field(
        WS_ELEMENT_FIELD_MAPPING,
        &t,
        WS_WSZ_TYPE,
        null(),
        core::mem::offset_of!(Answer, text),
        WS_FIELD_OPTIONAL,
    );
    let no_text_fields: [*const WsFieldDescription; 2] = [&f_optional_text, &f_number];
    let no_text = described::<Answer>(no_text_fields.as_ptr(), 2, 0);
    let mut value = Answer { text: usize::MAX as *mut u16, number: SENTINEL, attr: SENTINEL };
    let hr = read_struct(
        c,
        b"<r><n>42</n></r>",
        &no_text,
        WS_READ_REQUIRED_VALUE,
        (&raw mut value).cast(),
        size_of::<Answer>() as u32,
        heap,
        error,
    )?;
    c.hr(hr, S_OK)?;
    c.eq(value.text as u64, 0)?;
    c.eq(u64::from(value.number), 42)?;

    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);

    section(b"objects");
    let (heap, error) = objects(&mut c)?;
    section(b"errors");
    errors(&mut c, error)?;
    section(b"reading");
    reading(&mut c, heap, error)?;
    section(b"faulted");
    faulted(&mut c, heap, error)?;
    section(b"properties");
    properties(&mut c, heap, error)?;
    section(b"content");
    content(&mut c, heap, error)?;
    section(b"fragments");
    fragments(&mut c, heap, error)?;
    section(b"depth");
    depth(&mut c, error)?;
    section(b"structs");
    structs(&mut c, heap, error)?;

    // SAFETY: both objects were created above and nothing holds them.
    unsafe {
        WsFreeError(error);
        WsFreeHeap(heap);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok").s(CRLF).print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: the process ends here.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
