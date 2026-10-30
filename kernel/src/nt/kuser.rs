//! KUSER_SHARED_DATA: the read-only page every process sees at 0x7FFE0000.
//!
//! Field offsets are from the public KUSER_SHARED_DATA definition
//! (Microsoft documentation, WDK/mingw-w64 ntddk.h). Values describe the
//! pinned build (Windows 11 24H2, 26100). Time fields are KSYSTEM_TIMEs,
//! written High2Time, LowPart, High1Time so readers can detect tearing.

use super::vad::{self, Vad};
use crate::arch::paging::{SharedPage, UserSpace};
use crate::mm::paging::PageFlags;
use crate::sync::IrqMutex;
use crate::time;

pub const ADDRESS: u64 = 0x7ffe_0000;

const TICK_COUNT_LOW_DEPRECATED: usize = 0x000;
const TICK_COUNT_MULTIPLIER: usize = 0x004;
const INTERRUPT_TIME: usize = 0x008;
const SYSTEM_TIME: usize = 0x014;
const TIME_ZONE_BIAS: usize = 0x020;
const IMAGE_NUMBER_LOW: usize = 0x02c;
const IMAGE_NUMBER_HIGH: usize = 0x02e;
const NT_SYSTEM_ROOT: usize = 0x030;
const NT_BUILD_NUMBER: usize = 0x260;
const NT_PRODUCT_TYPE: usize = 0x264;
const PRODUCT_TYPE_IS_VALID: usize = 0x268;
const NATIVE_PROCESSOR_ARCHITECTURE: usize = 0x26a;
const NT_MAJOR_VERSION: usize = 0x26c;
const NT_MINOR_VERSION: usize = 0x270;
const PROCESSOR_FEATURES: usize = 0x274;
const NUMBER_OF_PHYSICAL_PAGES: usize = 0x2e8;
const TEST_RET_INSTRUCTION: usize = 0x2f8;
const QPC_FREQUENCY: usize = 0x300;
const TICK_COUNT: usize = 0x320;
const ACTIVE_PROCESSOR_COUNT: usize = 0x3c0;
const ACTIVE_GROUP_COUNT: usize = 0x3c4;

const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const PROCESSOR_ARCHITECTURE_AMD64: u16 = 9;
const NT_PRODUCT_WIN_NT: u32 = 1;
/// One tick = 1 ms: ms = TickCount * multiplier >> 24.
const TICK_MULTIPLIER: u32 = (1000 / time::TICK_HZ as u32) << 24;
const SYSTEM_ROOT: &str = "C:\\Windows";

/// ProcessorFeatures[] indices (PF_* in winnt.h) we can always report.
const PF_RDTSC_INSTRUCTION_AVAILABLE: usize = 8;
const PF_XMMI_INSTRUCTIONS_AVAILABLE: usize = 6;
const PF_XMMI64_INSTRUCTIONS_AVAILABLE: usize = 10;
const PF_NX_ENABLED: usize = 12;

pub const MAJOR_VERSION: u32 = 10;
pub const MINOR_VERSION: u32 = 0;
pub const BUILD_NUMBER: u32 = 26100;

static PAGE: IrqMutex<Option<SharedPage>> = IrqMutex::new(None);

fn write_ksystem_time(page: &SharedPage, offset: usize, value: u64) {
    let (low, high) = (value as u32, (value >> 32) as u32);
    page.write_u32(offset + 8, high); // High2Time
    page.write_u32(offset, low);
    page.write_u32(offset + 4, high); // High1Time
}

/// Creates the page and fills its constant fields.
pub fn init(physical_pages: u64) -> Result<(), crate::arch::paging::PagingError> {
    let page = SharedPage::new()?;
    page.write_u32(TICK_COUNT_MULTIPLIER, TICK_MULTIPLIER);
    write_ksystem_time(&page, TIME_ZONE_BIAS, 0); // UTC
    page.write(IMAGE_NUMBER_LOW, &IMAGE_FILE_MACHINE_AMD64.to_le_bytes());
    page.write(IMAGE_NUMBER_HIGH, &IMAGE_FILE_MACHINE_AMD64.to_le_bytes());
    for (i, b) in SYSTEM_ROOT.bytes().enumerate() {
        page.write(NT_SYSTEM_ROOT + 2 * i, &u16::from(b).to_le_bytes());
    }
    page.write_u32(NT_BUILD_NUMBER, BUILD_NUMBER);
    page.write_u32(NT_PRODUCT_TYPE, NT_PRODUCT_WIN_NT);
    page.write(PRODUCT_TYPE_IS_VALID, &[1]);
    page.write(NATIVE_PROCESSOR_ARCHITECTURE, &PROCESSOR_ARCHITECTURE_AMD64.to_le_bytes());
    page.write_u32(NT_MAJOR_VERSION, 10);
    page.write_u32(NT_MINOR_VERSION, 0);
    for pf in [
        PF_RDTSC_INSTRUCTION_AVAILABLE,
        PF_XMMI_INSTRUCTIONS_AVAILABLE,
        PF_XMMI64_INSTRUCTIONS_AVAILABLE,
        PF_NX_ENABLED,
    ] {
        page.write(PROCESSOR_FEATURES + pf, &[1]);
    }
    page.write_u32(NUMBER_OF_PHYSICAL_PAGES, physical_pages.min(u64::from(u32::MAX)) as u32);
    page.write(TEST_RET_INSTRUCTION, &[0xc3]); // ret
    page.write(QPC_FREQUENCY, &time::PERFORMANCE_FREQUENCY.to_le_bytes());
    page.write_u32(ACTIVE_PROCESSOR_COUNT, 1);
    page.write(ACTIVE_GROUP_COUNT, &[1]);
    update_page(&page, time::ticks());
    *PAGE.lock() = Some(page);
    time::set_tick_callback(on_tick);
    Ok(())
}

fn update_page(page: &SharedPage, ticks: u64) {
    write_ksystem_time(page, INTERRUPT_TIME, time::monotonic_ns() / 100);
    write_ksystem_time(page, SYSTEM_TIME, time::system_time());
    // TickCount: TickCountQuad (u64) overlapping a KSYSTEM_TIME.
    write_ksystem_time(page, TICK_COUNT, ticks);
    page.write_u32(TICK_COUNT_LOW_DEPRECATED, ticks as u32);
}

fn on_tick(ticks: u64) {
    if let Some(page) = PAGE.lock().as_ref() {
        update_page(page, ticks);
    }
}

/// Maps the page read-only into a new address space and records it in
/// the VAD.
pub fn map_into(space: &mut UserSpace, vad: &mut Vad) -> Result<(), super::status::NtStatus> {
    use super::status::NtStatus;
    let guard = PAGE.lock();
    let page = guard.as_ref().ok_or(NtStatus::NO_MEMORY)?;
    vad.reserve(Some(ADDRESS), 0x1000, vad::PAGE_READONLY, vad::MEM_PRIVATE, false)?;
    vad.commit(ADDRESS, 0x1000, vad::PAGE_READONLY)?;
    let flags = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RO };
    space.map_shared(ADDRESS, page, flags).map_err(|_| NtStatus::NO_MEMORY)
}
