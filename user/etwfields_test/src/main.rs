//! Every shape a TraceLogging field can have, written by a program and
//! read back by whoever is listening (ROADMAP OT4, OBSERVABILITY "A
//! session gala starts"): the decoder in the kernel has to take a
//! program's own bytes and say what they mean, and this is the program
//! whose bytes say what the answer must be.
//!
//! The events are built the way `TraceLoggingProvider.h` documents them,
//! and the builders here are that documentation in code: the provider's
//! name in a metadata blob of type 2, the event's name and its fields in
//! one of type 1 (each field a name, an in-type, an out-type when the
//! in-type's high bit says so, tag bytes when the out-type's does, and a
//! count when the in-type says the array's length is fixed), and the
//! values after them, end to end, with no padding between.
//!
//! Six events cover what a decoder must get right:
//!
//! - `Scalars`: the eight integer widths, signed and unsigned, with
//!   values that are wrong in an obvious way if a width or a sign is
//!   read wrong.
//! - `Numbers`: float, double and two BOOL32s, because a bool in ETW is
//!   four bytes and a 0 that prints as `true` is a bug one cannot see
//!   in production.
//! - `Texts`: the four string types -- nul-terminated wide and 8-bit,
//!   counted wide and counted 8-bit -- because their lengths are found
//!   in three different ways.
//! - `Bytes`: counted binary, a GUID, a FILETIME, and the two hex
//!   integers, whose in-type is the only thing that says to print them
//!   as hex.
//! - `Shapes`: a fixed-length array, a variable-length array, a struct
//!   of two fields, a field with a tag, and two fields whose out-type
//!   is what says how to print them.
//! - `Nothing`: an event with no fields at all, which must not read as
//!   an event whose fields were lost.
//!
//! What it prints is the same on real Windows and on gala, because it
//! prints what it wrote and what the provider was told, never what a
//! listener made of it: with no session running (real Windows, and gala
//! without `etw=`) no callback comes and every write is still accepted.
//! The decoded lines are the `etwfields_armed` scenario's business.
//!
//! Prints what it wrote and `etwfields_test: ok`, or
//! `etwfields_test: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt as _;

const NAME: &str = "etwfields_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

/// What ETW calls when a session's demands on a provider change.
type EnableCallback = unsafe extern "system" fn(
    source: *const [u8; 16],
    is_enabled: u32,
    level: u8,
    match_any: u64,
    match_all: u64,
    filter: *const c_void,
    context: *mut c_void,
);

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn EventRegister(
        provider: *const [u8; 16],
        callback: Option<EnableCallback>,
        context: *mut c_void,
        handle: *mut u64,
    ) -> u32;
    fn EventUnregister(handle: u64) -> u32;
    fn EventWrite(
        handle: u64,
        descriptor: *const EventDescriptor,
        count: u32,
        data: *const DataDescriptor,
    ) -> u32;
}

/// EVENT_DESCRIPTOR. A TraceLogging event's id is 0 and its channel is
/// 11: its name lives in its metadata, not in a manifest.
#[repr(C)]
struct EventDescriptor {
    id: u16,
    version: u8,
    channel: u8,
    level: u8,
    opcode: u8,
    task: u16,
    keyword: u64,
}

impl EventDescriptor {
    const fn new(level: u8, keyword: u64) -> Self {
        Self { id: 0, version: 0, channel: 11, level, opcode: 0, task: 0, keyword }
    }
}

/// EVENT_DATA_DESCRIPTOR. `kind` is 0 for the event's own data, 1 for
/// the event metadata and 2 for the provider metadata.
#[repr(C)]
struct DataDescriptor {
    ptr: u64,
    size: u32,
    kind: u32,
}

impl DataDescriptor {
    fn new(bytes: &[u8], kind: u32) -> Self {
        Self { ptr: bytes.as_ptr() as u64, size: bytes.len() as u32, kind }
    }
}

/// The provider, and the GUID its name hashes to by the algorithm
/// `TraceLoggingProvider.h` documents (d0b5081d-9af0-5ab0-5d8b-82289048fa05),
/// so a run can ask for it by name.
const PROVIDER_NAME: &str = "Gala.Test.Fields";
const PROVIDER: [u8; 16] = [
    0x1d, 0x08, 0xb5, 0xd0, 0xf0, 0x9a, 0xb0, 0x5a, 0x5d, 0x8b, 0x82, 0x28, 0x90, 0x48, 0xfa, 0x05,
];

// The in-types, from `TraceLoggingProvider.h`'s TlgIn_t.
const IN_UNICODESTRING: u8 = 1;
const IN_ANSISTRING: u8 = 2;
const IN_INT8: u8 = 3;
const IN_UINT8: u8 = 4;
const IN_INT16: u8 = 5;
const IN_UINT16: u8 = 6;
const IN_INT32: u8 = 7;
const IN_UINT32: u8 = 8;
const IN_INT64: u8 = 9;
const IN_UINT64: u8 = 10;
const IN_FLOAT: u8 = 11;
const IN_DOUBLE: u8 = 12;
const IN_BOOL32: u8 = 13;
const IN_BINARY: u8 = 14;
const IN_GUID: u8 = 15;
const IN_FILETIME: u8 = 17;
const IN_HEXINT32: u8 = 20;
const IN_HEXINT64: u8 = 21;
const IN_COUNTEDSTRING: u8 = 22;
const IN_COUNTEDANSISTRING: u8 = 23;
const IN_STRUCT: u8 = 24;

/// A fixed-length array: the count is in the metadata.
const IN_CCOUNT: u8 = 32;
/// A variable-length array: a `u16` count stands before the values.
const IN_VCOUNT: u8 = 64;
/// An out-type byte follows the in-type.
const IN_CHAIN: u8 = 128;
/// Tag bytes follow the out-type.
const OUT_CHAIN: u8 = 128;

// The out-types this program uses, from TlgOut_t.
const OUT_HEX: u8 = 4;
const OUT_WIN32ERROR: u8 = 13;

/// A metadata blob under construction: the size goes in first and is
/// written again at the end, which is how both blob kinds are shaped.
struct Blob {
    bytes: [u8; 256],
    len: usize,
}

impl Blob {
    /// A provider's metadata: the size, then the name.
    fn provider(name: &str) -> Self {
        let mut blob = Self { bytes: [0; 256], len: 2 };
        blob.text(name);
        blob.size();
        blob
    }

    /// An event's metadata: the size, one tag byte, then the name. The
    /// tag byte's high bit is clear, so it is the only one.
    fn event(name: &str) -> Self {
        let mut blob = Self { bytes: [0; 256], len: 2 };
        blob.byte(0);
        blob.text(name);
        blob
    }

    fn byte(&mut self, b: u8) {
        self.bytes[self.len] = b;
        self.len += 1;
    }

    /// A name, in UTF-8 and nul-terminated, as every name in a blob is.
    fn text(&mut self, text: &str) {
        for &b in text.as_bytes() {
            self.byte(b);
        }
        self.byte(0);
    }

    /// A field of one value: its name and its in-type.
    fn field(&mut self, name: &str, in_type: u8) -> &mut Self {
        self.text(name);
        self.byte(in_type);
        self
    }

    /// A field whose out-type says how to print it.
    fn field_out(&mut self, name: &str, in_type: u8, out_type: u8) -> &mut Self {
        self.text(name);
        self.byte(in_type | IN_CHAIN);
        self.byte(out_type);
        self
    }

    /// A field with a tag: the out-type's high bit says the tag bytes
    /// follow it. One byte of tag holds the tag's top 7 bits.
    fn field_tagged(&mut self, name: &str, in_type: u8, out_type: u8, tag: u8) -> &mut Self {
        self.text(name);
        self.byte(in_type | IN_CHAIN);
        self.byte(out_type | OUT_CHAIN);
        self.byte(tag);
        self
    }

    /// A fixed-length array: the count is here and not in the data.
    fn field_array(&mut self, name: &str, in_type: u8, count: u16) -> &mut Self {
        self.text(name);
        self.byte(in_type | IN_CCOUNT);
        for b in count.to_le_bytes() {
            self.byte(b);
        }
        self
    }

    /// A variable-length array: a `u16` count stands before its values.
    fn field_list(&mut self, name: &str, in_type: u8) -> &mut Self {
        self.text(name);
        self.byte(in_type | IN_VCOUNT);
        self
    }

    /// A struct: it has no data of its own, and its out-type is how many
    /// of the fields that follow belong to it.
    fn field_struct(&mut self, name: &str, fields: u8) -> &mut Self {
        self.text(name);
        self.byte(IN_STRUCT | IN_CHAIN);
        self.byte(fields);
        self
    }

    /// The whole blob's size, written over the two bytes it began with.
    fn size(&mut self) -> &mut Self {
        let size = (self.len as u16).to_le_bytes();
        self.bytes[0] = size[0];
        self.bytes[1] = size[1];
        self
    }

    fn done(&mut self) -> &[u8] {
        self.size();
        &self.bytes[..self.len]
    }

    /// A blob that is already finished, as it stands.
    fn bytes_of(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// An event's values, end to end with no padding, in the order the
/// metadata names the fields.
struct Data {
    bytes: [u8; 512],
    len: usize,
}

impl Data {
    fn new() -> Self {
        Self { bytes: [0; 512], len: 0 }
    }

    fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        for &b in bytes {
            self.bytes[self.len] = b;
            self.len += 1;
        }
        self
    }

    /// A nul-terminated 8-bit string, as in-type 2 wants it.
    fn ansi(&mut self, text: &str) -> &mut Self {
        self.raw(text.as_bytes());
        self.raw(&[0])
    }

    /// A nul-terminated wide string, as in-type 1 wants it.
    fn wide(&mut self, text: &str) -> &mut Self {
        for c in text.chars() {
            self.raw(&(c as u16).to_le_bytes());
        }
        self.raw(&[0, 0])
    }

    /// A counted wide string: the count is in bytes, not characters.
    fn counted_wide(&mut self, text: &str) -> &mut Self {
        let bytes = text.chars().count() as u16 * 2;
        self.raw(&bytes.to_le_bytes());
        for c in text.chars() {
            self.raw(&(c as u16).to_le_bytes());
        }
        self
    }

    /// A counted 8-bit string, and the same shape for counted binary.
    fn counted(&mut self, bytes: &[u8]) -> &mut Self {
        self.raw(&(bytes.len() as u16).to_le_bytes());
        self.raw(bytes)
    }

    fn done(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// What the provider's enable callback was told, and how often it was
/// called: a provider learns a session's demands here and nowhere else.
static CALLS: AtomicU32 = AtomicU32::new(0);
static ENABLED: AtomicU32 = AtomicU32::new(0);
static LEVEL: AtomicU32 = AtomicU32::new(0);
static ANY: AtomicU64 = AtomicU64::new(0);
static ALL: AtomicU64 = AtomicU64::new(0);

unsafe extern "system" fn enabled(
    _source: *const [u8; 16],
    is_enabled: u32,
    level: u8,
    match_any: u64,
    match_all: u64,
    _filter: *const c_void,
    _context: *mut c_void,
) {
    CALLS.fetch_add(1, Ordering::Relaxed);
    ENABLED.store(is_enabled, Ordering::Relaxed);
    LEVEL.store(u32::from(level), Ordering::Relaxed);
    ANY.store(match_any, Ordering::Relaxed);
    ALL.store(match_all, Ordering::Relaxed);
}

struct Out {
    handle: Handle,
    buf: [u8; 1024],
    len: usize,
}

impl Out {
    fn new() -> Self {
        // SAFETY: STD_OUTPUT_HANDLE.
        Self { handle: unsafe { GetStdHandle(0xffff_fff5) }, buf: [0; 1024], len: 0 }
    }

    fn flush(&mut self) {
        if self.len == 0 {
            return;
        }
        let mut written = 0u32;
        // SAFETY: our buffer and our handle.
        unsafe { WriteFile(self.handle, self.buf.as_ptr(), self.len as u32, &mut written, 0) };
        self.len = 0;
    }
}

impl core::fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len == self.buf.len() {
                self.flush();
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
        Ok(())
    }
}

macro_rules! say {
    ($out:expr, $($arg:tt)*) => {{
        use core::fmt::Write as _;
        let _ = writeln!($out, $($arg)*);
        $out.flush();
    }};
}

/// The first check that failed, and what it got, so a failure says
/// which line to look at.
struct Checks {
    failed: u32,
    got: u64,
    line: u32,
}

impl Checks {
    fn is(&mut self, n: u32, got: u64, want: u64, line: u32) {
        if got != want && self.failed == 0 {
            self.failed = n;
            self.got = got;
            self.line = line;
        }
    }
}

// The values, named here so the lines the program prints and the bytes
// it writes cannot drift apart.
const I8: i8 = -8;
const U8: u8 = 200;
const I16: i16 = -1600;
const U16: u16 = 60000;
const I32: i32 = -32000;
const U32: u32 = 4_000_000_000;
const I64: i64 = -6_400_000_000;
const U64: u64 = 18_000_000_000_000_000_000;
const F32: f32 = 1.5;
const F64: f64 = -2.25;
const WIDE: &str = "wide string";
const ANSI: &str = "ansi string";
const COUNTED_WIDE: &str = "counted wide";
const COUNTED_ANSI: &str = "counted ansi";
const BINARY: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];
/// 11112222-3333-4444-5555-666677778888, which is those bytes read the
/// way a GUID is: three little-endian numbers and then eight bytes.
const GUID: [u8; 16] = [
    0x22, 0x22, 0x11, 0x11, 0x33, 0x33, 0x44, 0x44, 0x55, 0x55, 0x66, 0x66, 0x77, 0x77, 0x88, 0x88,
];
/// 2026-11-22 19:15:03 UTC, in 100 ns units since 1601.
const FILETIME: u64 = 134_398_485_030_000_000;
const FILETIME_IS: &str = "2026-11-22 19:15:03";
const HEX32: u32 = 0xdead_beef;
const HEX64: u64 = 0x0123_4567_89ab_cdef;
const ARRAY: [u32; 3] = [10, 20, 30];
const LIST: [u16; 4] = [1, 2, 3, 4];
const LEFT: i32 = -1;
const TOP: i32 = 2;
/// A field tag of 0x02400000: one byte, whose low 7 bits are the tag's
/// top 7 (the header's own worked example).
const TAG_BYTE: u8 = 0x12;
const TAGGED: u32 = 99;
const AS_HEX: u32 = 0x00c0_ffee;
const AS_ERROR: u32 = 5;

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mut out = Out::new();
    let mut checks = Checks { failed: 0, got: 0, line: 0 };

    let mut handle = 0u64;
    // SAFETY: our provider id, our callback and our output.
    let status =
        unsafe { EventRegister(&PROVIDER, Some(enabled), core::ptr::null_mut(), &mut handle) };
    checks.is(1, u64::from(status), 0, line!());
    if status != 0 {
        say!(out, "{NAME}: EventRegister failed {status}");
        say!(out, "{NAME}: check 1 got {status} (line {})", line!());
        // SAFETY: leaving.
        unsafe { ExitProcess(1) };
    }

    let calls = CALLS.load(Ordering::Relaxed);
    if calls == 0 {
        say!(out, "{NAME}: {PROVIDER_NAME} registered, nobody is listening");
    } else {
        say!(
            out,
            "{NAME}: {PROVIDER_NAME} registered, {calls} callback(s), enabled {} level {} any {:#x} all {:#x}",
            ENABLED.load(Ordering::Relaxed),
            LEVEL.load(Ordering::Relaxed),
            ANY.load(Ordering::Relaxed),
            ALL.load(Ordering::Relaxed)
        );
    }

    let provider_meta = Blob::provider(PROVIDER_NAME);
    let mut written = 0u32;

    // Scalars: every integer width, signed and unsigned.
    {
        let mut meta = Blob::event("Scalars");
        meta.field("i8", IN_INT8)
            .field("u8", IN_UINT8)
            .field("i16", IN_INT16)
            .field("u16", IN_UINT16)
            .field("i32", IN_INT32)
            .field("u32", IN_UINT32)
            .field("i64", IN_INT64)
            .field("u64", IN_UINT64);
        let mut data = Data::new();
        data.raw(&I8.to_le_bytes())
            .raw(&U8.to_le_bytes())
            .raw(&I16.to_le_bytes())
            .raw(&U16.to_le_bytes())
            .raw(&I32.to_le_bytes())
            .raw(&U32.to_le_bytes())
            .raw(&I64.to_le_bytes())
            .raw(&U64.to_le_bytes());
        let status = write(handle, 4, 0x1, provider_meta.bytes_of(), meta.done(), data.done());
        checks.is(2, u64::from(status), 0, line!());
        written += 1;
        say!(
            out,
            "{NAME}: Scalars i8={I8}, u8={U8}, i16={I16}, u16={U16}, i32={I32}, u32={U32}, i64={I64}, u64={U64}"
        );
    }

    // Numbers: a float, a double, and a BOOL32 each way.
    {
        let mut meta = Blob::event("Numbers");
        meta.field("f32", IN_FLOAT)
            .field("f64", IN_DOUBLE)
            .field("yes", IN_BOOL32)
            .field("no", IN_BOOL32);
        let mut data = Data::new();
        data.raw(&F32.to_le_bytes())
            .raw(&F64.to_le_bytes())
            .raw(&1u32.to_le_bytes())
            .raw(&0u32.to_le_bytes());
        let status = write(handle, 4, 0x2, provider_meta.bytes_of(), meta.done(), data.done());
        checks.is(3, u64::from(status), 0, line!());
        written += 1;
        say!(out, "{NAME}: Numbers f32={F32}, f64={F64}, yes=true, no=false");
    }

    // Texts: the four string types, whose lengths are found three ways.
    {
        let mut meta = Blob::event("Texts");
        meta.field("wide", IN_UNICODESTRING)
            .field("ansi", IN_ANSISTRING)
            .field("cwide", IN_COUNTEDSTRING)
            .field("cansi", IN_COUNTEDANSISTRING);
        let mut data = Data::new();
        data.wide(WIDE).ansi(ANSI).counted_wide(COUNTED_WIDE).counted(COUNTED_ANSI.as_bytes());
        let status = write(handle, 4, 0x4, provider_meta.bytes_of(), meta.done(), data.done());
        checks.is(4, u64::from(status), 0, line!());
        written += 1;
        say!(
            out,
            "{NAME}: Texts wide={WIDE:?}, ansi={ANSI:?}, cwide={COUNTED_WIDE:?}, cansi={COUNTED_ANSI:?}"
        );
    }

    // Bytes: counted binary, a GUID, a FILETIME and the hex integers.
    {
        let mut meta = Blob::event("Bytes");
        meta.field("blob", IN_BINARY)
            .field("id", IN_GUID)
            .field("when", IN_FILETIME)
            .field("hex32", IN_HEXINT32)
            .field("hex64", IN_HEXINT64);
        let mut data = Data::new();
        data.counted(&BINARY)
            .raw(&GUID)
            .raw(&FILETIME.to_le_bytes())
            .raw(&HEX32.to_le_bytes())
            .raw(&HEX64.to_le_bytes());
        let status = write(handle, 4, 0x8, provider_meta.bytes_of(), meta.done(), data.done());
        checks.is(5, u64::from(status), 0, line!());
        written += 1;
        say!(
            out,
            "{NAME}: Bytes blob=deadbeef, id=11112222-3333-4444-5555-666677778888, when={FILETIME} ({FILETIME_IS}), hex32={HEX32:#x}, hex64={HEX64:#x}"
        );
    }

    // Shapes: the two array kinds, a struct, a tag, and two out-types.
    {
        let mut meta = Blob::event("Shapes");
        meta.field_array("three", IN_UINT32, ARRAY.len() as u16)
            .field_list("list", IN_UINT16)
            .field_struct("rect", 2)
            .field("left", IN_INT32)
            .field("top", IN_INT32)
            .field_tagged("tagged", IN_UINT32, 0, TAG_BYTE)
            .field_out("ashex", IN_UINT32, OUT_HEX)
            .field_out("aserror", IN_UINT32, OUT_WIN32ERROR);
        let mut data = Data::new();
        for v in ARRAY {
            data.raw(&v.to_le_bytes());
        }
        data.raw(&(LIST.len() as u16).to_le_bytes());
        for v in LIST {
            data.raw(&v.to_le_bytes());
        }
        data.raw(&LEFT.to_le_bytes())
            .raw(&TOP.to_le_bytes())
            .raw(&TAGGED.to_le_bytes())
            .raw(&AS_HEX.to_le_bytes())
            .raw(&AS_ERROR.to_le_bytes());
        let status = write(handle, 4, 0x10, provider_meta.bytes_of(), meta.done(), data.done());
        checks.is(6, u64::from(status), 0, line!());
        written += 1;
        say!(
            out,
            "{NAME}: Shapes three=[10, 20, 30], list=[1, 2, 3, 4], rect={{left={LEFT}, top={TOP}}}, tagged={TAGGED}, ashex={AS_HEX:#x}, aserror={AS_ERROR}"
        );
    }

    // Nothing: an event with no fields, which must not read as one
    // whose fields were lost.
    {
        let mut meta = Blob::event("Nothing");
        let status = write(handle, 4, 0x20, provider_meta.bytes_of(), meta.done(), &[]);
        checks.is(7, u64::from(status), 0, line!());
        written += 1;
        say!(out, "{NAME}: Nothing");
    }

    say!(out, "{NAME}: {written} events written, all accepted");

    // SAFETY: our registration.
    let status = unsafe { EventUnregister(handle) };
    checks.is(8, u64::from(status), 0, line!());

    if checks.failed != 0 {
        say!(out, "{NAME}: check {} got {} (line {})", checks.failed, checks.got, checks.line);
        // SAFETY: leaving.
        unsafe { ExitProcess(checks.failed) };
    }
    say!(out, "{NAME}: ok");
    // SAFETY: leaving.
    unsafe { ExitProcess(0) };
}

/// One event: the provider's metadata, the event's, then the values, in
/// the three descriptor kinds ETW expects them in.
fn write(
    handle: u64,
    level: u8,
    keyword: u64,
    provider_meta: &[u8],
    event_meta: &[u8],
    data: &[u8],
) -> u32 {
    let descriptor = EventDescriptor::new(level, keyword);
    let descriptors = [
        DataDescriptor::new(provider_meta, 2),
        DataDescriptor::new(event_meta, 1),
        DataDescriptor::new(data, 0),
    ];
    // An event with no fields has no data descriptor at all, which is
    // what the header's own writer does.
    let count = if data.is_empty() { 2 } else { 3 };
    // SAFETY: our registration, our descriptor and our own buffers,
    // which outlive the call.
    unsafe { EventWrite(handle, &descriptor, count, descriptors.as_ptr()) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
