//! A request's functions: WinHttpSendRequest / WriteData /
//! ReceiveResponse / QueryDataAvailable / ReadData / QueryHeaders over
//! the engine's request (`http_client::request`).

use alloc::vec::Vec;

use crate::{ERROR_INVALID_PARAMETER, done, request_of, wide};

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpSendRequest(
    h: usize,
    headers: *const u16,
    headers_length: u32,
    optional: *const u8,
    optional_length: u32,
    total_length: u32,
    _context: usize,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        let headers = if headers.is_null() || headers_length == 0 {
            Vec::new()
        } else {
            wide(
                headers,
                if headers_length == u32::MAX { None } else { Some(headers_length as usize) },
            )?
        };
        let optional: &[u8] = if optional.is_null() || optional_length == 0 {
            &[]
        } else {
            // SAFETY: the caller's buffer of `optional_length` bytes.
            unsafe { core::slice::from_raw_parts(optional, optional_length as usize) }
        };
        r.send(&headers, optional, total_length.into())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpWriteData(
    h: usize,
    data: *const u8,
    len: u32,
    written: *mut u32,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        if data.is_null() && len > 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let data: &[u8] = if len == 0 {
            &[]
        } else {
            // SAFETY: the caller's buffer of `len` bytes.
            unsafe { core::slice::from_raw_parts(data, len as usize) }
        };
        r.write_data(data)?;
        if !written.is_null() {
            // SAFETY: the caller's DWORD.
            unsafe { *written = len };
        }
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpReceiveResponse(h: usize, reserved: usize) -> i32 {
    done((|| {
        if reserved != 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        request_of(h)?.receive()
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpQueryDataAvailable(h: usize, available: *mut u32) -> i32 {
    done((|| {
        let n = request_of(h)?.available()?;
        if !available.is_null() {
            // SAFETY: the caller's DWORD.
            unsafe { *available = n.min(u32::MAX as usize) as u32 };
        }
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpReadData(h: usize, buffer: *mut u8, len: u32, read: *mut u32) -> i32 {
    done((|| {
        let r = request_of(h)?;
        if buffer.is_null() && len > 0 {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let out: &mut [u8] = if len == 0 {
            &mut []
        } else {
            // SAFETY: the caller's buffer of `len` bytes.
            unsafe { core::slice::from_raw_parts_mut(buffer, len as usize) }
        };
        let n = r.read_body(out)?;
        if !read.is_null() {
            // SAFETY: the caller's DWORD.
            unsafe { *read = n as u32 };
        }
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn WinHttpQueryHeaders(
    h: usize,
    level: u32,
    name: *const u16,
    buffer: *mut u8,
    length: *mut u32,
    index: *mut u32,
) -> i32 {
    done((|| {
        let r = request_of(h)?;
        let name = if level & 0xffff == http_client::request::QUERY_CUSTOM {
            if name.is_null() {
                return Err(ERROR_INVALID_PARAMETER);
            }
            wide(name, None)?
        } else {
            Vec::new()
        };
        // SAFETY: the caller's length, buffer and index, as documented.
        unsafe { r.query_info(level, &name, buffer, length, index) }
    })())
}
