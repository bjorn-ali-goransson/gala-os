//! Building the process: the image at its base, then the PEB, process
//! parameters, TEB, KUSER_SHARED_DATA and the main thread's stack at the
//! addresses gala's kernel uses (`layout`), filled as the kernel fills
//! them (kernel/src/nt/process.rs, thread.rs, params.rs).

use crate::host::{self, PROT_R, PROT_RW, PROT_RWX, PROT_RX};
use crate::layout::{self, params, peb, teb};
use crate::pe::PeImage;
use std::sync::OnceLock;

const PAGE: u64 = 4096;
const KUSER_SHARED_DATA: u64 = 0x7ffe_0000;
/// Windows' first process and thread ids (as gala numbers them).
pub const PROCESS_ID: u64 = 4;
pub const THREAD_ID: u64 = 8;
/// The console handles in the process parameters (as gala's kernel sets
/// them: StandardInput 4, StandardOutput 8, StandardError 12).
pub const STD_HANDLES: [u64; 3] = [4, 8, 12];

const SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const SCN_MEM_WRITE: u32 = 0x8000_0000;

/// Where and how the main thread starts.
pub struct Start {
    pub entry: u64,
    pub stack: u64,
    pub peb: u64,
    pub teb: u64,
}

static NAME: OnceLock<String> = OnceLock::new();

/// The program's file name (for the exit line).
pub fn name() -> &'static str {
    NAME.get().map_or("?", String::as_str)
}

fn round_up(n: u64) -> u64 {
    n.div_ceil(PAGE) * PAGE
}

pub fn load(name: &str, data: &[u8]) -> Result<Start, String> {
    let _ = NAME.set(name.to_string());
    let image = PeImage::parse(data).map_err(|e| format!("{e:?}"))?;
    if image.has_imports() {
        // U2b: imports need the NT layer's loader (ntdll, kernel32, ...).
        return Err("images with imports are not supported yet (U2b)".into());
    }
    let base = image.image_base;
    let size = round_up(u64::from(image.size_of_image));
    // SAFETY: a fresh mapping at an address nothing else uses (refused
    // otherwise); written below while this thread is the only one.
    let mem = unsafe { host::map_fixed(base, size, PROT_RW)? };
    image.lay_out(mem);
    // Section protections, as the kernel maps them.
    host::protect(base, round_up(u64::from(image.size_of_headers)), PROT_R)?;
    for s in image.sections() {
        let prot = match (
            s.characteristics & SCN_MEM_EXECUTE != 0,
            s.characteristics & SCN_MEM_WRITE != 0,
        ) {
            (true, true) => PROT_RWX,
            (true, false) => PROT_RX,
            (false, true) => PROT_RW,
            (false, false) => PROT_R,
        };
        let len = round_up(u64::from(s.memory_size()));
        if len > 0 {
            host::protect(base + u64::from(s.virtual_address), len, prot)?;
        }
    }

    // Main thread stack: [STACK_TOP - STACK_SIZE, STACK_TOP), all committed
    // for now (guard pages and demand commit come with the NT layer).
    let stack_bottom = layout::STACK_TOP - layout::STACK_SIZE;
    // SAFETY: as above.
    unsafe { host::map_fixed(stack_bottom, layout::STACK_SIZE, PROT_RW)? };

    // SAFETY: as above, for each structure.
    let peb_mem = unsafe { host::map_fixed(layout::PEB_ADDRESS, peb::PAGES * PAGE, PROT_RW)? };
    let params_mem =
        unsafe { host::map_fixed(layout::PARAMS_ADDRESS, params::PAGES * PAGE, PROT_RW)? };
    let teb_mem =
        unsafe { host::map_fixed(layout::FIRST_TEB_ADDRESS, teb::PAGES * PAGE, PROT_RW)? };
    let kuser = unsafe { host::map_fixed(KUSER_SHARED_DATA, PAGE, PROT_RW)? };
    kuser.fill(0);
    host::protect(KUSER_SHARED_DATA, PAGE, PROT_R)?;

    let put = |mem: &mut [u8], offset: u64, value: &[u8]| {
        mem[offset as usize..offset as usize + value.len()].copy_from_slice(value);
    };
    put(peb_mem, peb::IMAGE_BASE_ADDRESS, &base.to_le_bytes());
    put(peb_mem, peb::PROCESS_PARAMETERS, &layout::PARAMS_ADDRESS.to_le_bytes());
    put(peb_mem, peb::NUMBER_OF_PROCESSORS, &1u32.to_le_bytes());
    put(peb_mem, peb::IMAGE_SUBSYSTEM, &u32::from(image.subsystem).to_le_bytes());

    put(params_mem, params::MAXIMUM_LENGTH, &(params::STRUCT_SIZE as u32).to_le_bytes());
    put(params_mem, params::LENGTH, &(params::STRUCT_SIZE as u32).to_le_bytes());
    put(params_mem, params::FLAGS, &params::FLAG_NORMALIZED.to_le_bytes());
    for (i, h) in STD_HANDLES.iter().enumerate() {
        put(params_mem, params::STANDARD_INPUT + 8 * i as u64, &h.to_le_bytes());
    }

    let teb_addr = layout::FIRST_TEB_ADDRESS;
    put(teb_mem, teb::STACK_BASE, &layout::STACK_TOP.to_le_bytes());
    put(teb_mem, teb::STACK_LIMIT, &stack_bottom.to_le_bytes());
    put(teb_mem, teb::DEALLOCATION_STACK, &stack_bottom.to_le_bytes());
    put(teb_mem, teb::SELF, &teb_addr.to_le_bytes());
    put(teb_mem, teb::CLIENT_ID_PROCESS, &PROCESS_ID.to_le_bytes());
    put(teb_mem, teb::CLIENT_ID_THREAD, &THREAD_ID.to_le_bytes());
    put(teb_mem, teb::PEB, &layout::PEB_ADDRESS.to_le_bytes());

    eprintln!(
        "[info ] process {name} (pid {PROCESS_ID}): starting at {:#x}",
        base + u64::from(image.entry_rva)
    );
    Ok(Start {
        entry: base + u64::from(image.entry_rva),
        // As the kernel starts it: as if called (rsp % 16 == 8), with the
        // 32-byte shadow area above a null return address.
        stack: layout::STACK_TOP - 0x28,
        peb: layout::PEB_ADDRESS,
        teb: teb_addr,
    })
}
