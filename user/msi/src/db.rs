//! Reading an installer database (msiquery.h), which is what Office's
//! integrator.exe asks of msi.dll when it configures the Click-to-Run
//! MSIs: it opens `C2RInt.16.msi`, asks whether a table is there, and
//! reads the whole of `Feature` and `Property` row by row. The reading
//! itself is galapkg's MSI reader (`galapkg::msi`, the compound file,
//! the string pool, the catalog and a table read whole); what this
//! module adds is what the Windows Installer API is made of: handles,
//! views, a cursor and records.
//!
//! What is here is reading. A query that changes a database opens as a
//! view and fails when it runs, which is Windows' own answer for a
//! database opened read-only; gala has no engine to write one at all,
//! so that is its answer for the write modes too, and the attempt is
//! recorded in the ledger (ROADMAP OF). So is a query gala cannot
//! answer (one with a WHERE or an ORDER BY, which Windows runs): it is
//! refused as ERROR_BAD_QUERY_SYNTAX, and the record says the query was
//! not understood rather than wrong.
//!
//! Every answer below was measured on the host's own Windows 11 build
//! 26200 (`user/msi_test/src/db.rs`), down to the three that surprise:
//! a field past the record's last reads as empty rather than failing,
//! MsiRecordGetStringW reports the length it needs when it is given no
//! buffer, and a fetch past the last row rewinds the cursor.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;

use galapkg::msi::{Package, Value};

use crate::{
    ERROR_FILE_NOT_FOUND, ERROR_INSTALL_PACKAGE_INVALID, ERROR_INSTALL_PACKAGE_OPEN_FAILED,
    ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_ITEMS, ERROR_OPEN_FAILED,
    ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, record_gap,
};

/// ERROR_NOT_ENOUGH_MEMORY, ERROR_MORE_DATA, ERROR_BAD_QUERY_SYNTAX and
/// ERROR_FUNCTION_FAILED (winerror.h): the heap failed, the buffer is
/// too small, the query is not one, and the view cannot do that.
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_BAD_QUERY_SYNTAX: u32 = 1615;
const ERROR_FUNCTION_FAILED: u32 = 1627;

/// MSICONDITION (msiquery.h): the table is there and persistent, there
/// is no such table, or the handle is not a database.
const MSICONDITION_NONE: i32 = 2;
const MSICONDITION_TRUE: i32 = 1;
const MSICONDITION_ERROR: i32 = 3;

/// What MsiRecordGetInteger answers for a field that holds no integer.
const MSI_NULL_INTEGER: i32 = i32::MIN;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn AcquireSRWLockExclusive(lock: *mut usize);
    fn ReleaseSRWLockExclusive(lock: *mut usize);
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn GetFileSizeEx(file: usize, size: *mut i64) -> i32;
    fn ReadFile(file: usize, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetLastError() -> u32;
}

/// A value in a row, as a record holds it.
#[derive(Clone)]
enum Cell {
    /// No value, which is also what a stream column reads as here (gala
    /// does not hand out streams: MsiRecordReadStream is not exported).
    Null,
    Int(i32),
    Str(String),
}

impl Cell {
    /// The text MsiRecordGetStringW gives for it: an integer in decimal,
    /// nothing for no value.
    fn text(&self) -> Result<String, u32> {
        let mut s = String::new();
        match self {
            Cell::Null => {}
            Cell::Str(text) => {
                s.try_reserve_exact(text.len()).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
                s.push_str(text);
            }
            Cell::Int(i) => {
                // -2147483648 is the longest.
                s.try_reserve_exact(11).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
                let mut n = i.unsigned_abs();
                let mut digits = [0u8; 10];
                let mut at = digits.len();
                loop {
                    at -= 1;
                    digits[at] = b'0' + (n % 10) as u8;
                    n /= 10;
                    if n == 0 {
                        break;
                    }
                }
                if *i < 0 {
                    s.push('-');
                }
                for &d in &digits[at..] {
                    s.push(char::from(d));
                }
            }
        }
        Ok(s)
    }
}

/// An open database: its bytes, read whole when it was opened (an MSI
/// is a compound file, read all over), and whether the mode asked to
/// change it.
struct Database {
    data: Arc<Vec<u8>>,
}

/// A query that this module understands: a whole table, or named
/// columns of it in the order asked, or one that would change the
/// database.
struct Query {
    table: String,
    /// Empty for `*`, which is every column in the table's order.
    columns: Vec<String>,
    /// INSERT, UPDATE, DELETE and the rest: a view opens, running it
    /// does not.
    writes: bool,
}

/// An open view: the query, the database it came from, and the rows
/// once it has run.
struct View {
    database: u32,
    query: Query,
    rows: Option<Vec<Vec<Cell>>>,
    cursor: usize,
}

/// A row handed to the caller.
struct Record {
    fields: Vec<Cell>,
}

enum Object {
    Database(Database),
    View(View),
    Record(Record),
}

/// The live handles: a slot's index plus one is its MSIHANDLE (so 0 is
/// never a handle, as msiquery.h has it), and a slot is reused once its
/// handle is closed. An SRW lock guards the table; the objects are only
/// ever touched with it held.
struct Handles {
    lock: UnsafeCell<usize>,
    slots: UnsafeCell<Vec<Option<Object>>>,
}

// SAFETY: `slots` is only read or written with `lock` (an SRW lock)
// held, and nothing borrowed from it outlives that.
unsafe impl Sync for Handles {}

static HANDLES: Handles = Handles { lock: UnsafeCell::new(0), slots: UnsafeCell::new(Vec::new()) };

impl Handles {
    fn with<R>(&self, f: impl FnOnce(&mut Vec<Option<Object>>) -> R) -> R {
        // SAFETY: the SRW lock serialises every use of the table.
        unsafe {
            AcquireSRWLockExclusive(self.lock.get());
            let r = f(&mut *self.slots.get());
            ReleaseSRWLockExclusive(self.lock.get());
            r
        }
    }
}

/// Lists `object` and gives it a handle.
fn insert(object: Object) -> Result<u32, u32> {
    HANDLES.with(|slots| {
        if let Some(i) = slots.iter().position(Option::is_none) {
            slots[i] = Some(object);
            return Ok(i as u32 + 1);
        }
        if slots.len() == u32::MAX as usize {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        slots.try_reserve(1).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        slots.push(Some(object));
        Ok(slots.len() as u32)
    })
}

/// `f` over the object a handle names, or ERROR_INVALID_HANDLE.
fn with_object<R>(handle: u32, f: impl FnOnce(&mut Object) -> R) -> Result<R, u32> {
    if handle == 0 {
        return Err(ERROR_INVALID_HANDLE);
    }
    HANDLES.with(|slots| {
        let slot = slots.get_mut(handle as usize - 1).ok_or(ERROR_INVALID_HANDLE)?;
        Ok(f(slot.as_mut().ok_or(ERROR_INVALID_HANDLE)?))
    })
}

/// Closes a handle (MsiCloseHandle): 0 closes nothing and succeeds, a
/// handle that is open is forgotten, and anything else is invalid.
pub fn close(handle: u32) -> u32 {
    if handle == 0 {
        return ERROR_SUCCESS;
    }
    HANDLES.with(|slots| match slots.get_mut(handle as usize - 1) {
        Some(slot) if slot.is_some() => {
            *slot = None;
            // The table only grows while handles above it are open.
            while slots.last().is_some_and(Option::is_none) {
                slots.pop();
            }
            ERROR_SUCCESS
        }
        _ => ERROR_INVALID_HANDLE,
    })
}

/// Reads a file whole, for a database or package that is about to be
/// opened. GENERIC_READ with sharing, since the installer keeps its own
/// handles on the package. `opening` turns a failure to open the file
/// into the caller's answer: MsiOpenDatabaseW gives one answer however
/// the open failed, where MsiInstallProductW tells those apart
/// (`for_install`).
fn read_file(path: *const u16, opening: fn() -> u32) -> Result<Vec<u8>, u32> {
    // SAFETY: the caller's NUL-terminated path; FILE_SHARE_READ |
    // FILE_SHARE_WRITE, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL.
    let file = unsafe { CreateFileW(path, 0x8000_0000, 3, 0, 3, 0x80, 0) };
    if file == usize::MAX {
        return Err(opening());
    }
    let mut size = 0i64;
    // SAFETY: the handle just opened; our variable.
    let sized = unsafe { GetFileSizeEx(file, &mut size) };
    // A database is read whole, so one that cannot be is not one.
    let len = usize::try_from(size).ok().filter(|_| sized != 0);
    let mut data = Vec::new();
    let read = match len {
        None => Err(ERROR_OPEN_FAILED),
        Some(len) if data.try_reserve_exact(len).is_err() => Err(ERROR_NOT_ENOUGH_MEMORY),
        Some(len) => {
            data.resize(len, 0);
            let mut at = 0;
            let mut failed = None;
            while at < len {
                let want = u32::try_from(len - at).unwrap_or(u32::MAX);
                let mut got = 0;
                // SAFETY: the handle just opened; our buffer, from
                // where the last read stopped.
                let ok = unsafe { ReadFile(file, data[at..].as_mut_ptr(), want, &mut got, 0) };
                if ok == 0 || got == 0 {
                    failed = Some(ERROR_OPEN_FAILED);
                    break;
                }
                at += got as usize;
            }
            failed.map_or(Ok(()), Err)
        }
    };
    // SAFETY: the handle just opened, no longer used.
    unsafe { CloseHandle(file) };
    read.map(|()| data)
}

/// MsiOpenDatabaseW's work once the arguments are known good: the file
/// read, parsed as far as its catalog, and listed as a handle.
pub fn open(path: *const u16) -> Result<u32, u32> {
    let data = read_file(path, || ERROR_OPEN_FAILED)?;
    // A file that is not an installer database is ERROR_OPEN_FAILED, as
    // a file that is not there is: the same answer Windows gives, and
    // the reason the parse failed is the caller's to care about, not
    // gala's gap, so it is not recorded.
    Package::open(&data).map_err(|_| ERROR_OPEN_FAILED)?;
    insert(Object::Database(Database { data: Arc::new(data) }))
}

/// MsiDatabaseIsTablePersistentW (#165): whether the database holds
/// such a table. The names are the package's own, so `_Tables` and the
/// other streams the engine keeps are no table (MSICONDITION_NONE), and
/// the comparison is case-sensitive.
pub fn is_table_persistent(database: u32, table: *const u16) -> i32 {
    let Some(name) = from_wide(table) else {
        return MSICONDITION_ERROR;
    };
    let data = match with_object(database, |o| match o {
        Object::Database(db) => Some(Arc::clone(&db.data)),
        _ => None,
    }) {
        Ok(Some(data)) => data,
        _ => return MSICONDITION_ERROR,
    };
    let Ok(package) = Package::open(&data) else {
        return MSICONDITION_ERROR;
    };
    if package.tables().contains(&name) { MSICONDITION_TRUE } else { MSICONDITION_NONE }
}

/// MsiDatabaseOpenViewW (#32): a view over a query. The table and the
/// columns must be the database's, which is why this opens the package:
/// a query naming neither is ERROR_BAD_QUERY_SYNTAX, as it is on
/// Windows, and so is one gala cannot answer (recorded as such).
///
/// The handle is answered for before the caller's view is: a handle
/// that is not an open database leaves it as it was, and only past that
/// is it set to 0 and then to the view (measured; `msi_test`).
pub fn open_view(database: u32, query: *const u16, view: *mut u32) -> u32 {
    if view.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let data = match with_object(database, |o| match o {
        Object::Database(db) => Some(Arc::clone(&db.data)),
        _ => None,
    }) {
        Ok(Some(data)) => data,
        Ok(None) => return ERROR_INVALID_HANDLE,
        Err(e) => return e,
    };
    // SAFETY: the caller's handle, not null.
    unsafe { *view = 0 };
    let Some(text) = from_wide(query) else {
        return ERROR_INVALID_PARAMETER;
    };
    let Some(parsed) = parse(&text) else {
        record_gap(b"MsiDatabaseOpenViewW", b": query not understood", 2, 32);
        return ERROR_BAD_QUERY_SYNTAX;
    };
    if !parsed.writes {
        let Ok(package) = Package::open(&data) else {
            return ERROR_FUNCTION_FAILED;
        };
        let Some(columns) = package.columns(&parsed.table) else {
            return ERROR_BAD_QUERY_SYNTAX;
        };
        if !parsed.columns.iter().all(|want| columns.iter().any(|c| c.name == *want)) {
            return ERROR_BAD_QUERY_SYNTAX;
        }
    }
    match insert(Object::View(View { database, query: parsed, rows: None, cursor: 0 })) {
        Ok(handle) => {
            // SAFETY: the caller's handle, not null.
            unsafe { *view = handle };
            ERROR_SUCCESS
        }
        Err(e) => e,
    }
}

/// MsiViewExecute (#159): runs the query, which here means reading the
/// table and keeping the rows, with the cursor before the first.
/// Running a view again starts it over.
pub fn execute(view: u32, record: u32) -> u32 {
    if record != 0 {
        // A record stands for the `?` marks in a query, which none of
        // the queries gala answers has.
        record_gap(b"MsiViewExecute", b": query parameters", 2, 159);
    }
    let found = with_object(view, |o| match o {
        Object::View(v) => {
            Some((v.database, v.query.table.clone(), v.query.columns.clone(), v.query.writes))
        }
        _ => None,
    });
    let (database, table, columns, writes) = match found {
        Ok(Some(parts)) => parts,
        Ok(None) => return ERROR_INVALID_HANDLE,
        Err(e) => return e,
    };
    if writes {
        // Windows refuses this too for a database opened read-only; for
        // the modes that may write, gala has nothing to write with.
        record_gap(b"MsiViewExecute", b": writing a database", 2, 159);
        return ERROR_FUNCTION_FAILED;
    }
    let data = match with_object(database, |o| match o {
        Object::Database(db) => Some(Arc::clone(&db.data)),
        _ => None,
    }) {
        Ok(Some(data)) => data,
        Ok(None) => return ERROR_INVALID_HANDLE,
        Err(e) => return e,
    };
    let rows = match read_rows(&data, &table, &columns) {
        Ok(rows) => rows,
        Err(e) => return e,
    };
    // The view may have been closed while the table was read, which is
    // the caller's business (as on Windows); its handle is gone then.
    match with_object(view, |o| match o {
        Object::View(v) => {
            v.rows = Some(rows);
            v.cursor = 0;
            ERROR_SUCCESS
        }
        _ => ERROR_INVALID_HANDLE,
    }) {
        Ok(status) => status,
        Err(e) => e,
    }
}

/// The rows the query asks for, as cells of their own.
fn read_rows(data: &[u8], table: &str, columns: &[String]) -> Result<Vec<Vec<Cell>>, u32> {
    let package = Package::open(data).map_err(|_| ERROR_FUNCTION_FAILED)?;
    let read = package.table(table).map_err(|_| ERROR_FUNCTION_FAILED)?;
    let mut picked = Vec::new();
    if columns.is_empty() {
        picked.try_reserve_exact(read.columns.len()).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        picked.extend(0..read.columns.len());
    } else {
        picked.try_reserve_exact(columns.len()).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        for want in columns {
            picked.push(read.column(want).ok_or(ERROR_BAD_QUERY_SYNTAX)?);
        }
    }
    let mut rows = Vec::new();
    rows.try_reserve_exact(read.rows.len()).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
    let mut streams = false;
    for row in &read.rows {
        let mut fields = Vec::new();
        fields.try_reserve_exact(picked.len()).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        for &at in &picked {
            fields.push(match row.get(at) {
                Some(Value::Int(i)) => Cell::Int(*i),
                Some(Value::Str(s)) => {
                    let mut text = String::new();
                    text.try_reserve_exact(s.len()).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
                    text.push_str(s);
                    Cell::Str(text)
                }
                Some(Value::Stream(_)) => {
                    streams = true;
                    Cell::Null
                }
                Some(Value::Null) | None => Cell::Null,
            });
        }
        rows.push(fields);
    }
    if streams {
        record_gap(b"MsiViewFetch", b": a stream column reads as no value", 2, 160);
    }
    Ok(rows)
}

/// MsiViewFetch (#160): the next row, as a record. Past the last row it
/// is ERROR_NO_MORE_ITEMS **and the cursor goes back to the start**,
/// which is what Windows does (`msi_test`): the fetch after it gives
/// the first row again.
pub fn fetch(view: u32, record: *mut u32) -> u32 {
    if record.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's handle, not null.
    unsafe { *record = 0 };
    let row = with_object(view, |o| match o {
        Object::View(v) => match &v.rows {
            // A view that has not run has no rows to give.
            None => Err(ERROR_FUNCTION_FAILED),
            Some(rows) => match rows.get(v.cursor) {
                None => {
                    v.cursor = 0;
                    Err(ERROR_NO_MORE_ITEMS)
                }
                Some(row) => {
                    v.cursor += 1;
                    Ok(row.clone())
                }
            },
        },
        _ => Err(ERROR_INVALID_HANDLE),
    });
    let fields = match row {
        Ok(Ok(fields)) => fields,
        Ok(Err(status)) => return status,
        Err(e) => return e,
    };
    match insert(Object::Record(Record { fields })) {
        Ok(handle) => {
            // SAFETY: the caller's handle, not null.
            unsafe { *record = handle };
            ERROR_SUCCESS
        }
        Err(e) => e,
    }
}

/// MsiViewClose (#158): the view keeps its query and loses its rows, so
/// a fetch fails until it is run again. Closing a view twice is no
/// error; the handle itself stays open until MsiCloseHandle.
pub fn close_view(view: u32) -> u32 {
    match with_object(view, |o| match o {
        Object::View(v) => {
            v.rows = None;
            v.cursor = 0;
            ERROR_SUCCESS
        }
        _ => ERROR_INVALID_HANDLE,
    }) {
        Ok(status) => status,
        Err(e) => e,
    }
}

/// MsiRecordGetFieldCount (#115): how many fields the record has, or
/// 0xffffffff for a handle that is not a record (what Windows answers).
pub fn field_count(record: u32) -> u32 {
    with_object(record, |o| match o {
        Object::Record(r) => r.fields.len() as u32,
        _ => u32::MAX,
    })
    .unwrap_or(u32::MAX)
}

/// MsiRecordGetStringW (#118): the field's text. With no buffer it is
/// the length that is needed; with one too small it is
/// ERROR_MORE_DATA, the length needed, and as much of the text as
/// leaves room for the NUL (all measured on Windows).
pub fn get_string(record: u32, field: u32, buf: *mut u16, size: *mut u32) -> u32 {
    if size.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    let text = match with_object(record, |o| match o {
        // A field the record has not got is the empty string, not an
        // error: Windows answers so, and Office counts on it.
        Object::Record(r) => match field.checked_sub(1).and_then(|at| r.fields.get(at as usize)) {
            Some(cell) => cell.text(),
            None => Ok(String::new()),
        },
        _ => Err(ERROR_INVALID_HANDLE),
    }) {
        Ok(Ok(text)) => text,
        Ok(Err(e)) | Err(e) => return e,
    };
    let units = text.encode_utf16().count();
    let needed = u32::try_from(units).unwrap_or(u32::MAX);
    // SAFETY: the caller's size, not null.
    let room = unsafe { *size };
    // SAFETY: the caller's size, not null.
    unsafe { *size = needed };
    if buf.is_null() {
        return ERROR_SUCCESS;
    }
    // The NUL has to fit as well, so a buffer exactly as long as the
    // text is still too small.
    let fits = room as usize > units;
    let take = if fits { units } else { (room as usize).saturating_sub(1) };
    for (i, unit) in text.encode_utf16().take(take).enumerate() {
        // SAFETY: the caller's buffer, `room` units long, and `i` is
        // below `take`, which is below `room`.
        unsafe { buf.add(i).write(unit) };
    }
    if room > 0 {
        // SAFETY: as above, with room for the NUL kept back.
        unsafe { buf.add(take).write(0) };
    }
    if fits { ERROR_SUCCESS } else { ERROR_MORE_DATA }
}

/// MsiRecordGetInteger (#116): the field's integer, or MSI_NULL_INTEGER
/// for a field that holds no integer, one the record has not got, or a
/// handle that is not a record.
pub fn get_integer(record: u32, field: u32) -> i32 {
    with_object(record, |o| match o {
        Object::Record(r) => match field.checked_sub(1).and_then(|at| r.fields.get(at as usize)) {
            Some(Cell::Int(i)) => *i,
            _ => MSI_NULL_INTEGER,
        },
        _ => MSI_NULL_INTEGER,
    })
    .unwrap_or(MSI_NULL_INTEGER)
}

/// MsiRecordIsNull (#119): whether the field holds no value. A field
/// the record has not got holds none either; a handle that is not a
/// record answers the same, which is what Windows does (there is no
/// other answer to give).
pub fn is_null(record: u32, field: u32) -> i32 {
    with_object(record, |o| match o {
        Object::Record(r) => match field.checked_sub(1).and_then(|at| r.fields.get(at as usize)) {
            Some(Cell::Null) | None => 1,
            Some(_) => 0,
        },
        _ => 1,
    })
    .unwrap_or(1)
}

/// The caller's NUL-terminated UTF-16 string as a `String` (None for a
/// NULL pointer or a string the heap cannot hold).
fn from_wide(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut units = Vec::new();
    for i in 0.. {
        // SAFETY: the caller's string, read up to its NUL.
        let unit = unsafe { *p.add(i) };
        if unit == 0 {
            break;
        }
        units.try_reserve(1).ok()?;
        units.push(unit);
    }
    let mut out = String::new();
    out.try_reserve(units.len()).ok()?;
    for c in char::decode_utf16(units) {
        out.push(c.unwrap_or(char::REPLACEMENT_CHARACTER));
    }
    Some(out)
}

/// A query gala understands: `SELECT * FROM `T``, `SELECT `a`,`b` FROM
/// `T``, or one that would change the database. Keywords are
/// case-insensitive and the backquotes optional, both as Windows has
/// it; a query with anything more (a WHERE, an ORDER BY, a join) is
/// None, which the caller records and refuses.
fn parse(query: &str) -> Option<Query> {
    let (first, rest) = split_word(query.trim());
    if ["INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP"]
        .iter()
        .any(|w| first.eq_ignore_ascii_case(w))
    {
        return Some(Query { table: String::new(), columns: Vec::new(), writes: true });
    }
    if !first.eq_ignore_ascii_case("SELECT") {
        return None;
    }
    let (list, table) = split_from(rest)?;
    let mut columns = Vec::new();
    if list.trim() != "*" {
        for name in list.split(',') {
            let name = unquote(name.trim())?;
            let mut held = String::new();
            held.try_reserve_exact(name.len()).ok()?;
            held.push_str(name);
            columns.try_reserve(1).ok()?;
            columns.push(held);
        }
        if columns.is_empty() {
            return None;
        }
    }
    let table = unquote(table)?;
    let mut held = String::new();
    held.try_reserve_exact(table.len()).ok()?;
    held.push_str(table);
    Some(Query { table: held, columns, writes: false })
}

/// The first word and what follows it.
fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.find(char::is_whitespace) {
        Some(at) => (&text[..at], &text[at..]),
        None => (text, ""),
    }
}

/// The column list and the table name around the query's FROM, or None
/// when it has none, or when anything follows the table name (a WHERE,
/// an ORDER BY, a second table: all of which Windows runs and gala does
/// not).
fn split_from(rest: &str) -> Option<(&str, &str)> {
    let bytes = rest.as_bytes();
    for at in 0..bytes.len().saturating_sub(3) {
        if !bytes[at..at + 4].eq_ignore_ascii_case(b"FROM")
            || !(at == 0 || bytes[at - 1].is_ascii_whitespace())
            || !bytes.get(at + 4).is_none_or(u8::is_ascii_whitespace)
            || !rest.is_char_boundary(at)
        {
            continue;
        }
        let table = rest[at + 4..].trim();
        if table.is_empty() || table.contains(char::is_whitespace) || table.contains(',') {
            return None;
        }
        return Some((&rest[..at], table));
    }
    None
}

/// A name with its backquotes taken off, or None when the quoting is
/// not a name (an empty name, a quote that does not close, a space).
fn unquote(name: &str) -> Option<&str> {
    let bare = match name.strip_prefix('`') {
        Some(rest) => rest.strip_suffix('`')?,
        None => name,
    };
    if bare.is_empty() || bare.contains('`') || bare.contains(char::is_whitespace) {
        return None;
    }
    Some(bare)
}

/// What MsiInstallProductW (#88) makes of the package it is given:
/// `Ok(())` for one gala read as a package (which it still cannot
/// install: the caller records that gap), and otherwise Windows' own
/// answer for why no package could be read there, measured on build
/// 26200 (`msi_test`). Windows tells those apart where MsiOpenDatabaseW
/// does not: the file is not there (ERROR_FILE_NOT_FOUND), its
/// directory is not (ERROR_PATH_NOT_FOUND), it is a directory rather
/// than a file (ERROR_INSTALL_PACKAGE_OPEN_FAILED: CreateFileW refuses
/// one without FILE_FLAG_BACKUP_SEMANTICS, and so does every other
/// reason an open can fail), or it is a file that is not a package
/// (ERROR_INSTALL_PACKAGE_INVALID).
pub fn for_install(path: *const u16) -> Result<(), u32> {
    let data = read_file(path, || {
        // SAFETY: plain call, for the open that just failed.
        match unsafe { GetLastError() } {
            ERROR_FILE_NOT_FOUND => ERROR_FILE_NOT_FOUND,
            ERROR_PATH_NOT_FOUND => ERROR_PATH_NOT_FOUND,
            _ => ERROR_INSTALL_PACKAGE_OPEN_FAILED,
        }
    })
    .map_err(|e| if e == ERROR_OPEN_FAILED { ERROR_INSTALL_PACKAGE_INVALID } else { e })?;
    Package::open(&data).map(|_| ()).map_err(|_| ERROR_INSTALL_PACKAGE_INVALID)
}
