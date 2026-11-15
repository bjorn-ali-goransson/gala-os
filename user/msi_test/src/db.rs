//! Reading an installer database, which Office's integrator.exe does
//! with the six msi functions it looks up at run time
//! (`C2RManifest::ConfigureMSIs`): MsiOpenDatabase,
//! MsiDatabaseIsTablePersistentW, MsiDatabaseOpenViewW, MsiViewExecute,
//! MsiViewFetch and MsiRecordGetStringW, asking for the whole of the
//! `Feature` and `Property` tables. The database is galapkg's
//! `basic.msi` test package (wixl-built, checked in and used by `cargo
//! test -p galapkg`), which this program writes beside its own image
//! and opens.
//!
//! Every value here was measured on the owner's Windows 11 build
//! 26200.9974 (x64 and x86 answer alike), including the three that are
//! not what the documentation suggests:
//!
//! * A field past the record's last one is not an error: it reads as
//!   the empty string, and as MSI_NULL_INTEGER.
//! * MsiRecordGetStringW with no buffer and a size of 0 succeeds and
//!   reports the length; a size that is only as big as the length is
//!   ERROR_MORE_DATA, and fills the buffer with one character fewer and
//!   a NUL.
//! * A fetch past the last row answers ERROR_NO_MORE_ITEMS **and
//!   rewinds the cursor**: the fetch after it gives the first row
//!   again. Office's loops end on the ERROR_NO_MORE_ITEMS, so they
//!   never see this; a loop that asked once more would start over.

use core::ptr::null_mut;

use crate::{
    Checks, ERROR_INVALID_HANDLE, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, GetLastError,
    GetModuleFileNameW, MSIDBOPEN_DIRECT, MSIDBOPEN_READONLY, MSIDBOPEN_TRANSACT, MsiCloseHandle,
    MsiOpenDatabaseW, WriteFile, wide,
};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn CloseHandle(h: usize) -> i32;
    fn DeleteFileW(name: *const u16) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "msi", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "msi", kind = "raw-dylib"))]
unsafe extern "system" {
    fn MsiDatabaseIsTablePersistentW(database: u32, table: *const u16) -> i32;
    fn MsiDatabaseOpenViewW(database: u32, query: *const u16, view: *mut u32) -> u32;
    fn MsiViewExecute(view: u32, record: u32) -> u32;
    fn MsiViewFetch(view: u32, record: *mut u32) -> u32;
    fn MsiViewClose(view: u32) -> u32;
    fn MsiRecordGetFieldCount(record: u32) -> u32;
    fn MsiRecordGetStringW(record: u32, field: u32, buf: *mut u16, size: *mut u32) -> u32;
    fn MsiRecordGetInteger(record: u32, field: u32) -> i32;
    fn MsiRecordIsNull(record: u32, field: u32) -> i32;
}

const ERROR_MORE_DATA: u32 = 234;
const ERROR_BAD_QUERY_SYNTAX: u32 = 1615;
const ERROR_FUNCTION_FAILED: u32 = 1627;

/// MSICONDITION (msiquery.h): the table is there and persistent, or
/// there is no such table, or the handle is not a database.
const MSICONDITION_TRUE: i32 = 1;
const MSICONDITION_NONE: i32 = 2;
const MSICONDITION_ERROR: i32 = 3;

/// What MsiRecordGetInteger answers for a field that holds no integer.
const MSI_NULL_INTEGER: i32 = i32::MIN;

/// galapkg's test package: one feature, six properties and the tables
/// wixl puts in every package (`user/galapkg/testdata/msi/basic.wxs`,
/// built by `make_fixtures.sh`).
const BASIC: &[u8] = include_bytes!("../../galapkg/testdata/msi/basic.msi");

/// The name it is written under, beside this program's own image: the
/// one directory both gala and the host's Windows let this program
/// write to.
const FIXTURE: &str = "msi_test_basic.msi";

/// `basic.msi`'s `Property` table, in the order the rows come out.
const PROPERTIES: [&str; 6] = [
    "UpgradeCode",
    "Manufacturer",
    "ProductLanguage",
    "ProductCode",
    "ProductName",
    "ProductVersion",
];

/// The package's UpgradeCode, the one property value that is written in
/// `basic.wxs` rather than made up when it is built.
const UPGRADE_CODE: &str = "{11111111-2222-3333-4444-666666666666}";

/// A NUL-terminated UTF-16 path to `FIXTURE` beside this program.
fn fixture_path(c: &mut Checks) -> Result<[u16; 300], u32> {
    let mut path = [0u16; 300];
    // SAFETY: our buffer, with room left for the name we append.
    let n = unsafe { GetModuleFileNameW(0, path.as_mut_ptr(), 200) } as usize;
    c.ok(n > 0 && n < 200, n as u64)?;
    let cut = path[..n].iter().rposition(|&u| u == u16::from(b'\\')).map_or(0, |i| i + 1);
    for (slot, b) in path[cut..].iter_mut().zip(FIXTURE.bytes()) {
        *slot = u16::from(b);
    }
    path[cut + FIXTURE.len()] = 0;
    Ok(path)
}

/// Writes the package out (GENERIC_WRITE, CREATE_ALWAYS).
fn write_fixture(c: &mut Checks, path: &[u16]) -> Result<(), u32> {
    // SAFETY: a NUL-terminated path; no sharing, no template.
    let h = unsafe { CreateFileW(path.as_ptr(), 0x4000_0000, 0, 0, 2, 0x80, 0) };
    // SAFETY: plain call.
    c.ok(h != usize::MAX, u64::from(unsafe { GetLastError() }))?;
    let mut written = 0;
    // SAFETY: our buffer and its length; the handle just opened.
    let ok = unsafe { WriteFile(h, BASIC.as_ptr(), BASIC.len() as u32, &mut written, 0) };
    // SAFETY: plain calls.
    unsafe {
        let last = GetLastError();
        CloseHandle(h);
        c.ok(ok != 0, u64::from(last))?;
    }
    c.eq(written.into(), BASIC.len() as u64)
}

/// True when `buf` holds `text` and a NUL.
fn same(buf: &[u16], text: &str) -> bool {
    if buf.len() <= text.len() {
        return false;
    }
    buf.iter().zip(text.bytes()).all(|(&u, b)| u == u16::from(b)) && buf[text.len()] == 0
}

/// A string field: its length when asked with no buffer, then its text.
#[track_caller]
fn string_field(c: &mut Checks, record: u32, field: u32, want: &str) -> Result<(), u32> {
    let mut size = 0;
    // SAFETY: a record handle; no buffer, so the call only reports the
    // length, which it does without an error.
    c.eq(unsafe { MsiRecordGetStringW(record, field, null_mut(), &mut size) }.into(), 0)?;
    c.eq(size.into(), want.len() as u64)?;
    let mut buf = [0u16; 300];
    let mut size = buf.len() as u32;
    // SAFETY: our buffer and its length in units.
    c.eq(unsafe { MsiRecordGetStringW(record, field, buf.as_mut_ptr(), &mut size) }.into(), 0)?;
    c.eq(size.into(), want.len() as u64)?;
    c.ok(same(&buf, want), u64::from(size))
}

/// The one row of `Feature`: `Complete`, no parent, "Everything", no
/// description, display 2, level 1, no directory, attributes 0.
fn feature_row(c: &mut Checks, record: u32) -> Result<(), u32> {
    // SAFETY: a record handle.
    c.eq(unsafe { MsiRecordGetFieldCount(record) }.into(), 8)?;
    string_field(c, record, 1, "Complete")?;
    string_field(c, record, 2, "")?;
    string_field(c, record, 3, "Everything")?;
    string_field(c, record, 4, "")?;
    string_field(c, record, 5, "2")?;
    string_field(c, record, 6, "1")?;
    string_field(c, record, 7, "")?;
    string_field(c, record, 8, "0")?;
    let null_integer = u64::from(MSI_NULL_INTEGER as u32);
    // SAFETY: a record handle.
    unsafe {
        // A column that holds no value reads as null and as
        // MSI_NULL_INTEGER; a string column that holds one is not an
        // integer either.
        c.eq(u64::from(MsiRecordIsNull(record, 2) as u32), 1)?;
        c.eq(u64::from(MsiRecordIsNull(record, 3) as u32), 0)?;
        c.eq(u64::from(MsiRecordGetInteger(record, 2) as u32), null_integer)?;
        c.eq(u64::from(MsiRecordGetInteger(record, 3) as u32), null_integer)?;
        // The three integer columns.
        c.eq(u64::from(MsiRecordGetInteger(record, 5) as u32), 2)?;
        c.eq(u64::from(MsiRecordGetInteger(record, 6) as u32), 1)?;
        c.eq(u64::from(MsiRecordGetInteger(record, 8) as u32), 0)?;
        // A field the record has not got: the empty string, no error.
        let mut size = 0;
        c.eq(MsiRecordGetStringW(record, 9, null_mut(), &mut size).into(), 0)?;
        c.eq(size.into(), 0)?;
        c.eq(u64::from(MsiRecordGetInteger(record, 99) as u32), null_integer)?;
        c.eq(u64::from(MsiRecordIsNull(record, 99) as u32), 1)?;
    }
    // A buffer the text does not fit in: the length needed, and as much
    // of the text as leaves room for a NUL.
    let mut buf = [0u16; 300];
    let mut size = 5;
    // SAFETY: our buffer; a size smaller than it, which is what the
    // call goes by.
    c.eq(
        unsafe { MsiRecordGetStringW(record, 3, buf.as_mut_ptr(), &mut size) }.into(),
        ERROR_MORE_DATA.into(),
    )?;
    c.eq(size.into(), 10)?;
    c.ok(same(&buf, "Ever"), u64::from(size))?;
    // A buffer exactly as long as the text still has no room for a NUL.
    let mut size = 10;
    // SAFETY: as above.
    c.eq(
        unsafe { MsiRecordGetStringW(record, 3, buf.as_mut_ptr(), &mut size) }.into(),
        ERROR_MORE_DATA.into(),
    )?;
    c.eq(size.into(), 10)?;
    c.ok(same(&buf, "Everythin"), u64::from(size))?;
    // One unit more is enough.
    let mut size = 11;
    // SAFETY: as above.
    c.eq(unsafe { MsiRecordGetStringW(record, 3, buf.as_mut_ptr(), &mut size) }.into(), 0)?;
    c.eq(size.into(), 10)?;
    c.ok(same(&buf, "Everything"), u64::from(size))
}

/// The tables, views, cursors and records of a database that opens.
pub fn reading(c: &mut Checks) -> Result<(), u32> {
    let path = fixture_path(c)?;
    write_fixture(c, &path)?;
    let mut database = 0;
    // SAFETY: a NUL-terminated path; our handle.
    c.eq(
        unsafe { MsiOpenDatabaseW(path.as_ptr(), MSIDBOPEN_READONLY, &mut database) }.into(),
        ERROR_SUCCESS.into(),
    )?;
    c.ok(database != 0, database.into())?;

    // A table of the package, two tables of the engine itself, and
    // names that are neither (they are case-sensitive).
    for (table, want) in [
        ("Feature", MSICONDITION_TRUE),
        ("Property", MSICONDITION_TRUE),
        ("_Tables", MSICONDITION_NONE),
        ("_StringPool", MSICONDITION_NONE),
        ("NoSuchTable", MSICONDITION_NONE),
        ("feature", MSICONDITION_NONE),
        ("", MSICONDITION_NONE),
    ] {
        let name = wide::<16>(table);
        // SAFETY: a database handle and a NUL-terminated name.
        c.eq(
            u64::from(unsafe { MsiDatabaseIsTablePersistentW(database, name.as_ptr()) } as u32),
            want as u64,
        )?;
    }

    // A query that is not one: no view, and the same answer whether the
    // table, the column or the whole query is the problem.
    for query in
        ["SELECT * FROM `NoSuchTable`", "SELECT `NoSuchColumn` FROM `Feature`", "garbage", ""]
    {
        let sql = wide::<40>(query);
        let mut view = 0xdead_beef;
        // SAFETY: a database handle, a NUL-terminated query, our handle.
        c.eq(
            unsafe { MsiDatabaseOpenViewW(database, sql.as_ptr(), &mut view) }.into(),
            ERROR_BAD_QUERY_SYNTAX.into(),
        )?;
        c.eq(view.into(), 0)?;
    }

    // The whole of `Feature`, which is one of the integrator's two
    // queries.
    let feature = wide::<24>("SELECT * FROM `Feature`");
    let mut view = 0;
    // SAFETY: as above.
    c.eq(unsafe { MsiDatabaseOpenViewW(database, feature.as_ptr(), &mut view) }.into(), 0)?;
    c.ok(view != 0, view.into())?;

    let mut record = 0xdead_beef;
    // SAFETY: a view handle; our handle.
    unsafe {
        // A view that has not run has no rows to give.
        c.eq(MsiViewFetch(view, &mut record).into(), ERROR_FUNCTION_FAILED.into())?;
        c.eq(record.into(), 0)?;
        c.eq(MsiViewExecute(view, 0).into(), 0)?;
        c.eq(MsiViewFetch(view, &mut record).into(), 0)?;
        c.ok(record != 0, record.into())?;
    }
    feature_row(c, record)?;
    // SAFETY: the record and view handles.
    unsafe {
        c.eq(MsiCloseHandle(record).into(), 0)?;
        // The table holds one row; the fetch past it rewinds, so the
        // one after it gives that row again.
        record = 0xdead_beef;
        c.eq(MsiViewFetch(view, &mut record).into(), ERROR_NO_MORE_ITEMS.into())?;
        c.eq(record.into(), 0)?;
        c.eq(MsiViewFetch(view, &mut record).into(), 0)?;
    }
    string_field(c, record, 1, "Complete")?;
    // SAFETY: the record and view handles.
    unsafe {
        c.eq(MsiCloseHandle(record).into(), 0)?;
        // A closed view has no cursor; running it again gives it one.
        c.eq(MsiViewClose(view).into(), 0)?;
        c.eq(MsiViewClose(view).into(), 0)?;
        record = 0xdead_beef;
        c.eq(MsiViewFetch(view, &mut record).into(), ERROR_FUNCTION_FAILED.into())?;
        c.eq(record.into(), 0)?;
        c.eq(MsiViewExecute(view, 0).into(), 0)?;
        c.eq(MsiViewFetch(view, &mut record).into(), 0)?;
        c.eq(MsiCloseHandle(record).into(), 0)?;
        c.eq(MsiCloseHandle(view).into(), 0)?;
        c.eq(MsiCloseHandle(view).into(), ERROR_INVALID_HANDLE.into())?;
    }

    // The whole of `Property`, the integrator's other query: six rows
    // of two fields, in the order the table holds them.
    let property = wide::<25>("SELECT * FROM `Property`");
    let mut view = 0;
    // SAFETY: as above.
    unsafe {
        c.eq(MsiDatabaseOpenViewW(database, property.as_ptr(), &mut view).into(), 0)?;
        c.eq(MsiViewExecute(view, 0).into(), 0)?;
    }
    for name in PROPERTIES {
        let mut record = 0;
        // SAFETY: a view handle; our handle.
        unsafe {
            c.eq(MsiViewFetch(view, &mut record).into(), 0)?;
            c.eq(MsiRecordGetFieldCount(record).into(), 2)?;
        }
        string_field(c, record, 1, name)?;
        // SAFETY: a record handle.
        c.eq(unsafe { MsiCloseHandle(record) }.into(), 0)?;
    }
    let mut record = 0xdead_beef;
    // SAFETY: a view handle; our handle.
    unsafe {
        c.eq(MsiViewFetch(view, &mut record).into(), ERROR_NO_MORE_ITEMS.into())?;
        c.eq(record.into(), 0)?;
        // Running it again starts over.
        c.eq(MsiViewExecute(view, 0).into(), 0)?;
        c.eq(MsiViewFetch(view, &mut record).into(), 0)?;
    }
    string_field(c, record, 1, PROPERTIES[0])?;
    string_field(c, record, 2, UPGRADE_CODE)?;
    // SAFETY: the record and view handles.
    unsafe {
        c.eq(MsiCloseHandle(record).into(), 0)?;
        c.eq(MsiCloseHandle(view).into(), 0)?;
    }

    // Columns asked for by name come out in the order asked, which is
    // how such a query differs from a whole-table one: the fields are
    // the query's, not the table's.
    let columns = wide::<40>("SELECT `Level`,`Feature` FROM `Feature`");
    let mut view = 0;
    let mut record = 0;
    // SAFETY: as above.
    unsafe {
        c.eq(MsiDatabaseOpenViewW(database, columns.as_ptr(), &mut view).into(), 0)?;
        c.eq(MsiViewExecute(view, 0).into(), 0)?;
        c.eq(MsiViewFetch(view, &mut record).into(), 0)?;
        c.eq(MsiRecordGetFieldCount(record).into(), 2)?;
    }
    string_field(c, record, 1, "1")?;
    string_field(c, record, 2, "Complete")?;
    // SAFETY: the record and view handles.
    unsafe {
        c.eq(MsiCloseHandle(record).into(), 0)?;
        c.eq(MsiCloseHandle(view).into(), 0)?;
    }

    // A handle that is not open, and a database used as a view.
    let mut record = 0;
    let mut size = 0;
    // SAFETY: handles that are not records or views; our buffer.
    unsafe {
        c.eq(
            MsiRecordGetStringW(0xdead_beef, 1, null_mut(), &mut size).into(),
            ERROR_INVALID_HANDLE.into(),
        )?;
        c.eq(MsiRecordGetFieldCount(0xdead_beef).into(), u64::from(u32::MAX))?;
        c.eq(
            u64::from(MsiRecordGetInteger(0xdead_beef, 1) as u32),
            u64::from(MSI_NULL_INTEGER as u32),
        )?;
        c.eq(MsiViewFetch(0xdead_beef, &mut record).into(), ERROR_INVALID_HANDLE.into())?;
        c.eq(MsiViewExecute(0xdead_beef, 0).into(), ERROR_INVALID_HANDLE.into())?;
        c.eq(MsiViewExecute(database, 0).into(), ERROR_INVALID_HANDLE.into())?;
    }

    // Writing is not reading: the view opens, and running it on a
    // database opened read-only fails. gala has no engine to write a
    // database with at all, so that is its answer for the write modes
    // too (ROADMAP OF).
    let insert = wide::<61>("INSERT INTO `Property` (`Property`,`Value`) VALUES ('a','b')");
    let mut view = 0;
    // SAFETY: as above.
    unsafe {
        c.eq(MsiDatabaseOpenViewW(database, insert.as_ptr(), &mut view).into(), 0)?;
        c.eq(MsiViewExecute(view, 0).into(), ERROR_FUNCTION_FAILED.into())?;
        c.eq(MsiCloseHandle(view).into(), 0)?;
    }

    // A database handle that is closed is a handle like any other. The
    // view it is asked for is left as it was: the handle is answered
    // for before the out parameter is touched, which a query that is
    // not one is not (above, where the view reads 0).
    let name = wide::<8>("Feature");
    let mut view = 0xdead_beef;
    // SAFETY: a handle that is no longer open; our handles.
    unsafe {
        c.eq(MsiCloseHandle(database).into(), 0)?;
        c.eq(
            u64::from(MsiDatabaseIsTablePersistentW(database, name.as_ptr()) as u32),
            MSICONDITION_ERROR as u64,
        )?;
        c.eq(
            MsiDatabaseOpenViewW(database, feature.as_ptr(), &mut view).into(),
            ERROR_INVALID_HANDLE.into(),
        )?;
        c.eq(view.into(), 0xdead_beef)?;
    }

    // The two modes that open a database to change it open this one as
    // well (gala opens it to read, and refuses the writes). They are
    // asked here, with no other handle on the file: while one is open,
    // Windows refuses a second open of the same database - another
    // read-only one with ERROR_CREATE_FAILED, a writing one with
    // ERROR_OPEN_FAILED - which is bookkeeping of its own that gala does
    // not keep, so the test does not ask for it (ROADMAP OF).
    for mode in [MSIDBOPEN_TRANSACT, MSIDBOPEN_DIRECT] {
        let mut handle = 0;
        // SAFETY: a NUL-terminated path; our handle.
        unsafe {
            c.eq(MsiOpenDatabaseW(path.as_ptr(), mode, &mut handle).into(), 0)?;
            c.ok(handle != 0, handle.into())?;
            c.eq(MsiCloseHandle(handle).into(), 0)?;
        }
    }

    // SAFETY: a NUL-terminated path.
    let deleted = unsafe { DeleteFileW(path.as_ptr()) };
    // SAFETY: plain call.
    c.ok(deleted != 0, u64::from(unsafe { GetLastError() }))
}

/// The ordinals of the nine functions this module calls: integrator.exe
/// looks them up by name at run time, setup.exe imports them by
/// ordinal, and both find the same export (the host's msi.dll export
/// directory, build 26200).
pub const ORDINALS: [(u16, &core::ffi::CStr); 9] = [
    (32, c"MsiDatabaseOpenViewW"),
    (115, c"MsiRecordGetFieldCount"),
    (116, c"MsiRecordGetInteger"),
    (118, c"MsiRecordGetStringW"),
    (119, c"MsiRecordIsNull"),
    (158, c"MsiViewClose"),
    (159, c"MsiViewExecute"),
    (160, c"MsiViewFetch"),
    (165, c"MsiDatabaseIsTablePersistentW"),
];
