//! RTL_USER_PROCESS_PARAMETERS for a new process, built as bytes to copy
//! into its address space (normalized: string buffers hold absolute
//! addresses). Layout from the public x64 definition (see `layout::params`).

use super::platform;
use alloc::vec::Vec;

use super::layout::{self, params};
use crate::mm::heap::AllocError;

/// What goes into a process's parameters block.
pub struct ProcessParameters<'a> {
    pub image_path: &'a str,
    pub command_line: &'a str,
    /// With a trailing backslash, as Windows keeps it.
    pub current_directory: &'a str,
    pub current_directory_handle: u64,
    pub dll_path: &'a str,
    pub std_handles: [u64; 3],
    /// `NAME=value` strings.
    pub environment: &'a [&'a str],
    /// RuntimeData: STARTUPINFO's lpReserved2 bytes (empty: none).
    pub runtime_data: &'a [u8],
}

fn put(b: &mut [u8], off: u64, v: &[u8]) {
    let off = off as usize;
    b[off..off + v.len()].copy_from_slice(v);
}

/// Characters CurrentDirectory's buffer holds (MAX_PATH, as Windows
/// reserves it): SetCurrentDirectory writes the new path in place.
const CURRENT_DIRECTORY_CHARS: u64 = 260;

/// Appends `s` as a null-terminated UTF-16 string at `*cursor` (in a
/// buffer of at least `capacity` characters with the NUL) and stores its
/// UNICODE_STRING at `field`.
fn put_string(
    b: &mut Vec<u8>,
    base: u64,
    field: u64,
    cursor: &mut u64,
    s: &str,
    capacity: u64,
) -> Option<()> {
    let units = s.len() as u64;
    let room = (units + 1).max(capacity);
    let end = *cursor + 2 * room;
    if room * 2 > u64::from(u16::MAX) {
        return None;
    }
    grow(b, end).ok()?;
    for (i, c) in s.bytes().enumerate() {
        put(b, *cursor + 2 * i as u64, &u16::from(c).to_le_bytes());
    }
    put(b, field, &((units * 2) as u16).to_le_bytes()); // Length
    put(b, field + 2, &((room * 2) as u16).to_le_bytes()); // MaximumLength
    put(b, field + 8, &(base + *cursor).to_le_bytes());
    *cursor = end.next_multiple_of(8);
    Some(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParamsError {
    TooLarge,
    NoMemory,
}

impl From<AllocError> for ParamsError {
    fn from(_: AllocError) -> Self {
        ParamsError::NoMemory
    }
}

/// The most the block may take (it must end below the first TEB).
pub const MAX_SIZE: u64 = layout::FIRST_TEB_ADDRESS - layout::PARAMS_ADDRESS;

/// Makes `b` at least `len` bytes long (zeroed), within MAX_SIZE.
fn grow(b: &mut Vec<u8>, len: u64) -> Result<(), ParamsError> {
    if len > MAX_SIZE {
        return Err(ParamsError::TooLarge);
    }
    let len = len as usize;
    if b.len() < len {
        b.try_reserve(len - b.len()).map_err(|_| ParamsError::NoMemory)?;
        b.resize(len, 0);
    }
    Ok(())
}

/// Builds the block to be placed at user address `base`: the structure,
/// its strings, then the environment; a whole number of pages, as large
/// as they need (each string within a UNICODE_STRING's 32767 characters,
/// as on Windows, where a command line may be that long).
pub fn build(p: &ProcessParameters<'_>, base: u64) -> Result<Vec<u8>, ParamsError> {
    let mut b = Vec::new();
    grow(&mut b, params::STRUCT_SIZE)?;
    let mut cursor = params::STRUCT_SIZE;
    let strings = [
        (params::CURRENT_DIRECTORY, p.current_directory, CURRENT_DIRECTORY_CHARS),
        (params::DLL_PATH, p.dll_path, 0),
        (params::IMAGE_PATH_NAME, p.image_path, 0),
        (params::COMMAND_LINE, p.command_line, 0),
        (params::WINDOW_TITLE, p.image_path, 0),
    ];
    for (field, s, capacity) in strings {
        put_string(&mut b, base, field, &mut cursor, s, capacity).ok_or(ParamsError::TooLarge)?;
    }
    if !p.runtime_data.is_empty() {
        let n = p.runtime_data.len() as u64;
        if n > u64::from(u16::MAX) {
            return Err(ParamsError::TooLarge);
        }
        grow(&mut b, cursor + n)?;
        put(&mut b, cursor, p.runtime_data);
        put(&mut b, params::RUNTIME_DATA, &(n as u16).to_le_bytes());
        put(&mut b, params::RUNTIME_DATA + 2, &(n as u16).to_le_bytes());
        put(&mut b, params::RUNTIME_DATA + 8, &(base + cursor).to_le_bytes());
        cursor = (cursor + n).next_multiple_of(8);
    }
    put(&mut b, params::MAXIMUM_LENGTH, &(cursor as u32).to_le_bytes());
    put(&mut b, params::LENGTH, &(cursor as u32).to_le_bytes());
    put(&mut b, params::FLAGS, &params::FLAG_NORMALIZED.to_le_bytes());
    for (i, h) in p.std_handles.iter().enumerate() {
        put(&mut b, params::STANDARD_INPUT + 8 * i as u64, &h.to_le_bytes());
    }
    put(&mut b, params::CURRENT_DIRECTORY_HANDLE, &p.current_directory_handle.to_le_bytes());

    // Environment, after the strings: each string null-terminated, then
    // an empty string.
    let start = cursor.next_multiple_of(16);
    let mut at = start;
    for var in p.environment {
        let end = at + 2 * (var.len() as u64 + 1);
        grow(&mut b, end + 2)?;
        for (i, c) in var.bytes().enumerate() {
            put(&mut b, at + 2 * i as u64, &u16::from(c).to_le_bytes());
        }
        at = end;
    }
    grow(&mut b, at + 2)?;
    let env_size = at + 2 - start;
    put(&mut b, params::ENVIRONMENT, &(base + start).to_le_bytes());
    put(&mut b, params::ENVIRONMENT_SIZE, &env_size.to_le_bytes());
    // Whole pages: the block is mapped as it is.
    let whole = (b.len() as u64).next_multiple_of(platform::PAGE_SIZE);
    grow(&mut b, whole)?;
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(b: &[u8], off: u64) -> u16 {
        u16::from_le_bytes([b[off as usize], b[off as usize + 1]])
    }

    fn u64_at(b: &[u8], off: u64) -> u64 {
        u64::from_le_bytes(b[off as usize..off as usize + 8].try_into().unwrap())
    }

    /// Reads the UNICODE_STRING at `field` of a block placed at `base`.
    fn string(b: &[u8], base: u64, field: u64) -> String {
        let len = u64::from(u16_at(b, field));
        let ptr = u64_at(b, field + 8) - base;
        assert_eq!(u16_at(b, ptr + len), 0, "null-terminated");
        assert!(u64::from(u16_at(b, field + 2)) >= len + 2);
        (0..len / 2).map(|i| u16_at(b, ptr + 2 * i) as u8 as char).collect()
    }

    #[test]
    fn strings_environment_and_handles() {
        let base = 0x7ff0_0001_0000;
        let p = ProcessParameters {
            image_path: "C:\\gala\\x.exe",
            command_line: "C:\\gala\\x.exe arg",
            current_directory: "C:\\gala\\",
            current_directory_handle: 0x20,
            dll_path: "",
            std_handles: [4, 8, 0xc],
            environment: &["A=1", "SystemRoot=C:\\Windows"],
            runtime_data: &[1, 0, 0, 0, 0x41, 0x2c, 0, 0, 0, 0, 0, 0, 0],
        };
        let b = build(&p, base).unwrap();
        assert_eq!(string(&b, base, params::IMAGE_PATH_NAME), "C:\\gala\\x.exe");
        assert_eq!(string(&b, base, params::COMMAND_LINE), "C:\\gala\\x.exe arg");
        assert_eq!(string(&b, base, params::CURRENT_DIRECTORY), "C:\\gala\\");
        // MAX_PATH characters, for SetCurrentDirectory to write in place.
        assert_eq!(u16_at(&b, params::CURRENT_DIRECTORY + 2), 520);
        // RuntimeData: the bytes, with their length.
        assert_eq!(u16_at(&b, params::RUNTIME_DATA), 13);
        let at = (u64_at(&b, params::RUNTIME_DATA + 8) - base) as usize;
        assert_eq!(&b[at..at + 13], &[1, 0, 0, 0, 0x41, 0x2c, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(u64_at(&b, params::CURRENT_DIRECTORY_HANDLE), 0x20);
        assert_eq!(u64_at(&b, params::STANDARD_OUTPUT), 8);
        let length = u32::from_le_bytes(b[4..8].try_into().unwrap());
        assert!(u64::from(length) > params::STRUCT_SIZE);
        let env = u64_at(&b, params::ENVIRONMENT) - base;
        let units: Vec<u16> = (0..u64_at(&b, params::ENVIRONMENT_SIZE) / 2)
            .map(|i| u16_at(&b, env + 2 * i))
            .collect();
        let text: String =
            units.iter().map(|&c| if c == 0 { '|' } else { c as u8 as char }).collect();
        assert_eq!(text, "A=1|SystemRoot=C:\\Windows||");
    }

    #[test]
    fn long_command_lines_fit() {
        // Elasticsearch starts its server with a ~5000-character command
        // line; Windows allows 32767 characters.
        let line = "y".repeat(32_000);
        let p = ProcessParameters {
            image_path: "C:\\a.exe",
            command_line: &line,
            current_directory: "C:\\",
            current_directory_handle: 0,
            dll_path: "",
            std_handles: [0; 3],
            environment: &["A=1"],
            runtime_data: &[],
        };
        let base = 0x7ff0_0001_0000;
        let b = build(&p, base).unwrap();
        assert_eq!(b.len() as u64 % platform::PAGE_SIZE, 0);
        assert_eq!(string(&b, base, params::COMMAND_LINE).len(), 32_000);
        let env = u64_at(&b, params::ENVIRONMENT) - base;
        assert!(env >= 2 * 32_000);
        assert_eq!(u16_at(&b, env), u16::from(b'A'));
    }

    #[test]
    fn oversized_strings_are_rejected() {
        let long = "x".repeat(40_000);
        let p = ProcessParameters {
            image_path: &long,
            command_line: "",
            current_directory: "C:\\",
            current_directory_handle: 0,
            dll_path: "",
            std_handles: [0; 3],
            environment: &[],
            runtime_data: &[],
        };
        assert_eq!(build(&p, 0).unwrap_err(), ParamsError::TooLarge);
    }
}
