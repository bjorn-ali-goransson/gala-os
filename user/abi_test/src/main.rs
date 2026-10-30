//! Checks system-call error behaviour that is identical on real Windows.
//! Prints `abi_test: ok` and exits 0, or exits with the number of the
//! first failing check.

#![no_std]
#![no_main]

use ntrt::*;

fn check(id: u32, got: NtStatus, want: NtStatus) -> Result<(), u32> {
    if got == want {
        return Ok(());
    }
    print("abi_test: check ");
    write_file(stdout(), &hex32(id));
    print(" got ");
    write_file(stdout(), &hex32(got));
    print("\r\n");
    Err(id)
}

fn run() -> Result<(), u32> {
    // An out-of-range service number.
    check(1, invalid_service(), STATUS_INVALID_SYSTEM_SERVICE)?;
    // A handle value that was never opened.
    check(2, write_file(Handle(0x1230), b"x"), STATUS_INVALID_HANDLE)?;
    check(3, close(Handle(0x1230)), STATUS_INVALID_HANDLE)?;
    // Writing works and returns success.
    check(4, write_file(stdout(), b"abi_test: write ok\r\n"), STATUS_SUCCESS)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("abi_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
