//! RTL_USER_PROCESS_PARAMETERS for a new process, built as bytes to copy
//! into its address space (normalized: string buffers hold absolute
//! addresses). Layout from the public x64 definition (see `layout::params`).

use super::platform;
use alloc::vec::Vec;

use super::layout::params;
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

/// Appends `s` as a null-terminated UTF-16 string at `*cursor` and stores
/// its UNICODE_STRING at `field`.
fn put_string(b: &mut [u8], base: u64, field: u64, cursor: &mut u64, s: &str) -> Option<()> {
    let units = s.len() as u64;
    let end = *cursor + 2 * (units + 1);
    if end > params::ENVIRONMENT_OFFSET || units * 2 + 2 > u64::from(u16::MAX) {
        return None;
    }
    for (i, c) in s.bytes().enumerate() {
        put(b, *cursor + 2 * i as u64, &u16::from(c).to_le_bytes());
    }
    put(b, field, &((units * 2) as u16).to_le_bytes()); // Length
    put(b, field + 2, &((units * 2 + 2) as u16).to_le_bytes()); // MaximumLength
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

/// Builds the block to be placed at user address `base` (`PAGES` pages:
/// structure and strings, then the environment from `ENVIRONMENT_OFFSET`).
pub fn build(p: &ProcessParameters<'_>, base: u64) -> Result<Vec<u8>, ParamsError> {
    let total = (params::PAGES * platform::PAGE_SIZE) as usize;
    let mut b = Vec::new();
    b.try_reserve_exact(total).map_err(|_| ParamsError::NoMemory)?;
    b.resize(total, 0);
    let mut cursor = params::STRUCT_SIZE;
    let strings = [
        (params::CURRENT_DIRECTORY, p.current_directory),
        (params::DLL_PATH, p.dll_path),
        (params::IMAGE_PATH_NAME, p.image_path),
        (params::COMMAND_LINE, p.command_line),
        (params::WINDOW_TITLE, p.image_path),
    ];
    for (field, s) in strings {
        put_string(&mut b, base, field, &mut cursor, s).ok_or(ParamsError::TooLarge)?;
    }
    if !p.runtime_data.is_empty() {
        let n = p.runtime_data.len() as u64;
        if cursor + n > params::ENVIRONMENT_OFFSET || n > u64::from(u16::MAX) {
            return Err(ParamsError::TooLarge);
        }
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

    // Environment: each string null-terminated, then an empty string.
    let mut at = params::ENVIRONMENT_OFFSET;
    for var in p.environment {
        let end = at + 2 * (var.len() as u64 + 1);
        if end + 2 > total as u64 {
            return Err(ParamsError::TooLarge);
        }
        for (i, c) in var.bytes().enumerate() {
            put(&mut b, at + 2 * i as u64, &u16::from(c).to_le_bytes());
        }
        at = end;
    }
    let env_size = at + 2 - params::ENVIRONMENT_OFFSET;
    put(&mut b, params::ENVIRONMENT, &(base + params::ENVIRONMENT_OFFSET).to_le_bytes());
    put(&mut b, params::ENVIRONMENT_SIZE, &env_size.to_le_bytes());
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
        assert_eq!(u64::from(u16_at(b, field + 2)), len + 2);
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
    fn oversized_strings_are_rejected() {
        let long = "x".repeat(4000);
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
