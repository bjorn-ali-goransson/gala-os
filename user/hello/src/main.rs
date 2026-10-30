//! Prints a line through a raw `NtWriteFile` on the PEB's standard output
//! handle and exits with status 0.

#![no_std]
#![no_main]

fn main() -> u32 {
    ntrt::write_file(ntrt::stdout(), b"Hello from user mode!\r\n")
}

ntrt::entry!(main);
