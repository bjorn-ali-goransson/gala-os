//! The display driver's adapter enumeration (D3DKMT), which the Vulkan
//! loader calls through gdi32 to find a GPU: how many adapters there
//! are, what each one's handle and LUID is, and what
//! `D3DKMTQueryAdapterInfo` answers about each of them.
//!
//! What any Windows answers, measured on build 26300 with three
//! adapters:
//!
//! - `D3DKMTEnumAdapters2` with no structure at all:
//!   STATUS_INVALID_PARAMETER.
//! - with `pAdapters` NULL: STATUS_SUCCESS, and `NumAdapters` comes back
//!   as the room to make -- a capacity (34 there), not the count, and
//!   the same on every call.
//! - with room for that many: STATUS_SUCCESS, and `NumAdapters` is the
//!   number written. Asked again with more room, the same number.
//! - with room for fewer than there are: STATUS_BUFFER_TOO_SMALL
//!   (0xc0000023), and `NumAdapters` is left as the caller set it.
//! - `D3DKMTQueryAdapterInfo` wants the exact size of the structure its
//!   type names -- KMTQAITYPE_ADAPTERTYPE 4 bytes, UMDRIVERNAME 524,
//!   ADAPTERREGISTRYINFO 2080 -- and answers STATUS_INVALID_PARAMETER
//!   for any other size, for a type it does not know (9999), for no
//!   structure, for nowhere to put the answer, and for a handle it never
//!   issued.
//! - KMTQAITYPE_QUERYREGISTRY (48) with a D3DDDI_QUERYREGISTRY_INFO and
//!   no room after it for the value: STATUS_SUCCESS, with the
//!   structure's own `Status` =
//!   D3DDDI_QUERYREGISTRY_STATUS_BUFFER_OVERFLOW (1) and
//!   `OutputValueSize` the bytes the value needs (206 for the Vulkan
//!   driver's name there). Asked again with that much room after it:
//!   `Status` = SUCCESS (0), the same size, and the value itself (a
//!   driver store path ending in `.json`). A value the adapter's key has
//!   not got, an empty value name, and the service key in place of the
//!   adapter key: STATUS_OBJECT_NAME_NOT_FOUND (0xc0000034).
//!
//! Assumed of the machine it runs on: nothing. A machine with no display
//! driver (gala: GPU G2) answers the enumeration with no adapters and
//! refuses every handle, so the checks that need one are skipped; `info
//! adapters <n>` says how many it found and `info values <n>` how many
//! of those hold the Vulkan driver's name.
//!
//! Prints `d3dkmt_test: ok`, or `d3dkmt_test: check N failed (line L):
//! <what>, got 0x<value>` and exits with N.

#![no_std]
#![no_main]

use ntrt as _;

const NAME: &str = "d3dkmt_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn LoadLibraryA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> usize;
    fn ExitProcess(code: u32) -> !;
}

const STATUS_SUCCESS: i32 = 0;
const STATUS_INVALID_PARAMETER: i32 = 0xc000_000d_u32 as i32;
const STATUS_BUFFER_TOO_SMALL: i32 = 0xc000_0023_u32 as i32;
const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xc000_0034_u32 as i32;

/// The query types (KMTQAITYPE), and one no Windows knows.
const UMDRIVERNAME: u32 = 1;
const ADAPTERREGISTRYINFO: u32 = 8;
const ADAPTERTYPE: u32 = 15;
const QUERYREGISTRY: u32 = 48;
const NO_SUCH_TYPE: u32 = 9999;

/// The exact size each type's structure must be given.
const UMDRIVERNAME_SIZE: u32 = 524;
const ADAPTERREGISTRYINFO_SIZE: u32 = 2080;
const ADAPTERTYPE_SIZE: u32 = 4;

/// The keys a D3DDDI_QUERYREGISTRY_INFO can name, and its one flag.
const SERVICE_KEY: u32 = 0;
const ADAPTER_KEY: u32 = 1;
const TRANSLATE_PATH: u32 = 1;

/// Registry value types, the two the Vulkan loader asks for.
const REG_SZ: u32 = 1;
const REG_MULTI_SZ: u32 = 7;

/// D3DDDI_QUERYREGISTRY_STATUS_SUCCESS and _BUFFER_OVERFLOW: the
/// structure's own answer, beside the call's NTSTATUS.
const QUERY_SUCCESS: u32 = 0;
const BUFFER_OVERFLOW: u32 = 1;

/// A handle no D3DKMT call ever issues.
const NOT_AN_ADAPTER: u32 = 0xdead_beef;

/// D3DKMT_ADAPTERINFO: the handle, the LUID, how many video present
/// sources it has and whether it prefers precise present regions.
const ADAPTERINFO_SIZE: usize = 20;

/// As many adapters as this test makes room for.
const ADAPTERS: usize = 256;

/// D3DDDI_QUERYREGISTRY_INFO, and the offsets of the fields this test
/// sets or reads. The value follows the structure, so the structure's
/// own size is the "no room for the value" query.
const QRI_SIZE: usize = 552;
const QRI_QUERY_TYPE: usize = 0;
const QRI_QUERY_FLAGS: usize = 4;
const QRI_VALUE_NAME: usize = 8;
const QRI_VALUE_TYPE: usize = 528;
const QRI_OUTPUT_VALUE_SIZE: usize = 536;
const QRI_STATUS: usize = 540;
const QRI_OUTPUT: usize = 544;

/// Room for a value after the structure.
const QRI_VALUE_ROOM: usize = 1024;

#[repr(C)]
struct EnumAdapters2 {
    num_adapters: u32,
    _pad: u32,
    adapters: *mut u8,
}

impl EnumAdapters2 {
    fn new(num_adapters: u32, adapters: *mut u8) -> EnumAdapters2 {
        EnumAdapters2 { num_adapters, _pad: 0, adapters }
    }
}

#[repr(C)]
struct QueryAdapterInfo {
    adapter: u32,
    kind: u32,
    data: *mut u8,
    size: u32,
    _pad: u32,
}

type Enum2 = unsafe extern "system" fn(*mut EnumAdapters2) -> i32;
type Qai = unsafe extern "system" fn(*mut QueryAdapterInfo) -> i32;

/// A D3DDDI_QUERYREGISTRY_INFO with room for a value after it. Aligned
/// to 8, which the union at its end (a UINT64 among the output types)
/// asks for.
#[repr(C, align(8))]
struct Registry([u8; QRI_SIZE + QRI_VALUE_ROOM]);

impl Registry {
    /// A query of `key` for `value` of type `value_type`, with the paths
    /// it answers translated (what the Vulkan loader asks for).
    fn new(key: u32, value: &str, value_type: u32) -> Registry {
        let mut r = Registry([0; QRI_SIZE + QRI_VALUE_ROOM]);
        r.put(QRI_QUERY_TYPE, key);
        r.put(QRI_QUERY_FLAGS, TRANSLATE_PATH);
        r.put(QRI_VALUE_TYPE, value_type);
        // The names are ASCII; UTF-16 is one zero byte after each.
        for (i, b) in value.bytes().enumerate() {
            r.0[QRI_VALUE_NAME + 2 * i] = b;
        }
        r
    }

    fn put(&mut self, at: usize, v: u32) {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn get(&self, at: usize) -> u32 {
        u32at(&self.0, at)
    }

    /// Its own answer, beside the call's NTSTATUS.
    fn status(&self) -> u32 {
        self.get(QRI_STATUS)
    }

    /// The bytes the value needs, which an overflow fills in.
    fn value_size(&self) -> u32 {
        self.get(QRI_OUTPUT_VALUE_SIZE)
    }
}

/// Room for the largest structure a query type names, and for the sizes
/// either side of it.
#[repr(C, align(8))]
struct Big([u8; 2088]);

struct Line {
    buf: [u8; 400],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 400], n: 0 }
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

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    /// A wide string of the caller's, up to 160 characters, as ASCII.
    fn wide(mut self, w: &[u8]) -> Self {
        let mut i = 0;
        while i < 320 && i + 1 < w.len() {
            let c = u16::from_le_bytes([w[i], w[i + 1]]);
            if c == 0 {
                break;
            }
            let c = if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' };
            if self.n < self.buf.len() {
                self.buf[self.n] = c;
                self.n += 1;
            }
            i += 2;
        }
        self
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

fn info(what: &[u8], v: u64) {
    Line::new().s(NAME.as_bytes()).s(b": info ").s(what).s(b" ").dec(v).s(b"\r\n").print();
}

fn info_text(what: &[u8], w: &[u8]) {
    Line::new().s(NAME.as_bytes()).s(b": info ").s(what).s(b" ").wide(w).s(b"\r\n").print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64, what: &[u8]) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" failed (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b"): ")
            .s(what)
            .s(b", got ")
            .hex(got)
            .s(b"\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64, what: &[u8]) -> Result<(), u32> {
        self.ok(got == want, got, what)
    }

    /// An NTSTATUS, which is what almost every answer here is.
    #[track_caller]
    fn ret(&mut self, got: i32, want: i32, what: &[u8]) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64, what)
    }
}

fn u32at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64at(b: &[u8], at: usize) -> u64 {
    u64::from(u32at(b, at)) | (u64::from(u32at(b, at + 4)) << 32)
}

/// `D3DKMTQueryAdapterInfo` for `adapter`, asking `kind` into `size`
/// bytes at `data`.
fn query(qai: Qai, adapter: u32, kind: u32, data: *mut u8, size: u32) -> i32 {
    let mut q = QueryAdapterInfo { adapter, kind, data, size, _pad: 0 };
    // SAFETY: the structure is ours, and `data` holds `size` bytes (or
    // is null, which is one of the things asked about).
    unsafe { qai(&mut q) }
}

/// What `adapter` answers about itself: the sizes it insists on, the
/// types it refuses, and the Vulkan driver's name in its registry key
/// (1 in the return if it has one).
fn one_adapter(c: &mut Checks, qai: Qai, adapter: u32) -> Result<u32, u32> {
    let mut kind = [0u8; 4];
    let mut big = Big([0; 2088]);

    c.ret(
        query(qai, adapter, ADAPTERTYPE, kind.as_mut_ptr(), ADAPTERTYPE_SIZE),
        STATUS_SUCCESS,
        b"the adapter's type",
    )?;
    c.ret(
        query(qai, adapter, ADAPTERTYPE, kind.as_mut_ptr(), 0),
        STATUS_INVALID_PARAMETER,
        b"the adapter's type with no room",
    )?;
    c.ret(
        query(qai, adapter, ADAPTERTYPE, big.0.as_mut_ptr(), 64),
        STATUS_INVALID_PARAMETER,
        b"the adapter's type with too much room",
    )?;
    c.ret(
        query(qai, adapter, ADAPTERTYPE, core::ptr::null_mut(), ADAPTERTYPE_SIZE),
        STATUS_INVALID_PARAMETER,
        b"the adapter's type with nowhere to put it",
    )?;
    c.ret(
        query(qai, adapter, NO_SUCH_TYPE, kind.as_mut_ptr(), ADAPTERTYPE_SIZE),
        STATUS_INVALID_PARAMETER,
        b"a query type no Windows knows",
    )?;

    for (kind, what, size, may_be_absent) in [
        (UMDRIVERNAME, &b"the user-mode driver's name"[..], UMDRIVERNAME_SIZE, false),
        (
            ADAPTERREGISTRYINFO,
            &b"the adapter's registry information"[..],
            ADAPTERREGISTRYINFO_SIZE,
            true,
        ),
    ] {
        // An adapter with no registry key of its own answers
        // STATUS_OBJECT_NAME_NOT_FOUND where the others answer with the
        // information: of the three adapters on build 26300, the one
        // that has no Vulkan driver either answers so. The driver's
        // name is there for all three.
        let got = query(qai, adapter, kind, big.0.as_mut_ptr(), size);
        c.ok(
            got == STATUS_SUCCESS || (may_be_absent && got == STATUS_OBJECT_NAME_NOT_FOUND),
            got as u32 as u64,
            what,
        )?;
        for off in [-4i32, 4] {
            c.ret(
                query(qai, adapter, kind, big.0.as_mut_ptr(), (size as i32 + off) as u32),
                STATUS_INVALID_PARAMETER,
                what,
            )?;
        }
    }

    for (key, value, what) in [
        (ADAPTER_KEY, "GalaNoSuchValue", &b"a value the adapter's key has not got"[..]),
        (ADAPTER_KEY, "", &b"a query with no value name"[..]),
        (SERVICE_KEY, "VulkanDriverName", &b"the Vulkan driver's name under the service key"[..]),
    ] {
        let mut r = Registry::new(key, value, REG_SZ);
        c.ret(
            query(qai, adapter, QUERYREGISTRY, r.0.as_mut_ptr(), QRI_SIZE as u32),
            STATUS_OBJECT_NAME_NOT_FOUND,
            what,
        )?;
    }

    // The value itself, in the two types the Vulkan loader asks for (it
    // tries REG_MULTI_SZ and then REG_SZ; which of them a driver wrote
    // is the driver's business).
    for value_type in [REG_MULTI_SZ, REG_SZ] {
        let mut r = Registry::new(ADAPTER_KEY, "VulkanDriverName", value_type);
        let s = query(qai, adapter, QUERYREGISTRY, r.0.as_mut_ptr(), QRI_SIZE as u32);
        if s != STATUS_SUCCESS || r.status() != BUFFER_OVERFLOW {
            continue;
        }
        let want = r.value_size();
        c.ok(
            want > 0 && want as usize <= QRI_VALUE_ROOM,
            u64::from(want),
            b"the bytes the Vulkan driver's name needs",
        )?;
        let mut full = Registry::new(ADAPTER_KEY, "VulkanDriverName", value_type);
        c.ret(
            query(qai, adapter, QUERYREGISTRY, full.0.as_mut_ptr(), QRI_SIZE as u32 + want),
            STATUS_SUCCESS,
            b"the Vulkan driver's name, with room for it",
        )?;
        c.eq(u64::from(full.status()), u64::from(QUERY_SUCCESS), b"the value is there")?;
        // The overflow asks for room, not for the length: on build
        // 26300 it asked for 206 bytes and then wrote 190 of them.
        c.ok(
            full.value_size() > 0 && full.value_size() <= want,
            u64::from(full.value_size()),
            b"no more than the bytes it asked room for",
        )?;
        c.ok(full.0[QRI_OUTPUT] != 0, u64::from(full.0[QRI_OUTPUT]), b"the value is not empty")?;
        info_text(b"the Vulkan driver's name", &full.0[QRI_OUTPUT..]);
        return Ok(1);
    }
    Ok(0)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);

    // SAFETY: a name of ours, and the module it names.
    let gdi32 = unsafe { LoadLibraryA(b"gdi32.dll\0".as_ptr()) };
    c.ok(gdi32 != 0, gdi32 as u64, b"gdi32.dll is loaded")?;
    // SAFETY: the module, and names of ours.
    let (enum2, qai) = unsafe {
        (
            GetProcAddress(gdi32, b"D3DKMTEnumAdapters2\0".as_ptr()),
            GetProcAddress(gdi32, b"D3DKMTQueryAdapterInfo\0".as_ptr()),
        )
    };
    c.ok(enum2 != 0, enum2 as u64, b"gdi32 exports D3DKMTEnumAdapters2")?;
    c.ok(qai != 0, qai as u64, b"gdi32 exports D3DKMTQueryAdapterInfo")?;
    // SAFETY: the export, with the signature its documentation gives.
    let enum2 = unsafe { core::mem::transmute::<usize, Enum2>(enum2) };
    // SAFETY: as above.
    let qai = unsafe { core::mem::transmute::<usize, Qai>(qai) };

    // The one argument is the whole question.
    // SAFETY: no structure is what is being asked about.
    c.ret(unsafe { enum2(core::ptr::null_mut()) }, STATUS_INVALID_PARAMETER, b"no structure")?;

    // No array: how much room to make. A capacity, not the count.
    let mut ask = EnumAdapters2::new(0, core::ptr::null_mut());
    // SAFETY: our structure, with no array for it to fill.
    c.ret(unsafe { enum2(&mut ask) }, STATUS_SUCCESS, b"the room to make")?;
    let room = ask.num_adapters;
    info(b"room", u64::from(room));
    let mut again = EnumAdapters2::new(0, core::ptr::null_mut());
    // SAFETY: as above.
    c.ret(unsafe { enum2(&mut again) }, STATUS_SUCCESS, b"the room to make, asked again")?;
    c.eq(u64::from(again.num_adapters), u64::from(room), b"the same room as the first answer")?;
    c.ok(room as usize <= ADAPTERS, u64::from(room), b"the room fits this test's array")?;

    let mut list = [0u8; ADAPTERS * ADAPTERINFO_SIZE];
    let mut fill = EnumAdapters2::new(room, list.as_mut_ptr());
    // SAFETY: our structure, and an array with room for `room` of them.
    c.ret(unsafe { enum2(&mut fill) }, STATUS_SUCCESS, b"the adapters, in the room it asked for")?;
    let found = fill.num_adapters;
    c.ok(found <= room, u64::from(found), b"no more adapters than the room it asked for")?;
    info(b"adapters", u64::from(found));

    let mut roomier = EnumAdapters2::new(ADAPTERS as u32, list.as_mut_ptr());
    // SAFETY: our structure, and an array with room for ADAPTERS of them.
    c.ret(unsafe { enum2(&mut roomier) }, STATUS_SUCCESS, b"the adapters, in more room")?;
    c.eq(
        u64::from(roomier.num_adapters),
        u64::from(found),
        b"the same adapters however much room there is",
    )?;

    if found > 1 {
        let mut short = EnumAdapters2::new(found - 1, list.as_mut_ptr());
        // SAFETY: our structure, and an array with room for all of them,
        // of which it is told there is room for one fewer -- which is
        // what is being asked about.
        c.ret(unsafe { enum2(&mut short) }, STATUS_BUFFER_TOO_SMALL, b"one adapter short of room")?;
        c.eq(
            u64::from(short.num_adapters),
            u64::from(found - 1),
            b"the count left as the caller set it",
        )?;
    }

    // A handle no call issued, which answers the same on a machine with
    // adapters and on one without.
    // SAFETY: no structure is what is being asked about.
    c.ret(
        unsafe { qai(core::ptr::null_mut()) },
        STATUS_INVALID_PARAMETER,
        b"a query with no structure",
    )?;
    let mut kind = [0u8; 4];
    c.ret(
        query(qai, NOT_AN_ADAPTER, ADAPTERTYPE, kind.as_mut_ptr(), ADAPTERTYPE_SIZE),
        STATUS_INVALID_PARAMETER,
        b"the type of an adapter that was never issued",
    )?;
    let mut r = Registry::new(ADAPTER_KEY, "VulkanDriverName", REG_SZ);
    c.ret(
        query(qai, NOT_AN_ADAPTER, QUERYREGISTRY, r.0.as_mut_ptr(), QRI_SIZE as u32),
        STATUS_INVALID_PARAMETER,
        b"the registry of an adapter that was never issued",
    )?;

    let mut with_value = 0;
    for i in 0..found as usize {
        let at = i * ADAPTERINFO_SIZE;
        let adapter = u32at(&list, at);
        let luid = u64at(&list, at + 4);
        c.ok(adapter != 0, u64::from(adapter), b"an adapter's handle")?;
        c.ok(luid != 0, luid, b"an adapter's LUID")?;
        with_value += one_adapter(&mut c, qai, adapter)?;
    }
    info(b"values", u64::from(with_value));
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
