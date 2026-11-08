//! KUSER_SHARED_DATA's x64 parts: the machine (AMD64), the processor
//! features and the XState configuration (the XSAVE components the
//! platform enables, in the standard layout).

use crate::nt::kuser::*;
use crate::nt::platform::SharedPage;

const PROCESSOR_FEATURES: usize = 0x274;
const TEST_RET_INSTRUCTION: usize = 0x2f8;
/// XSTATE_CONFIGURATION XState (winnt.h's layout; at 0x3d8 since Windows
/// 10): EnabledFeatures, EnabledVolatileFeatures, Size, ControlFlags,
/// Features[64] (offset, size), EnabledSupervisorFeatures,
/// AlignedFeatures, AllFeatureSize, AllFeatures[64],
/// EnabledUserVisibleSupervisorFeatures, ExtendedFeatureDisableFeatures,
/// AllNonLargeFeatureSize.
const XSTATE: usize = 0x3d8;
const XSTATE_ENABLED_FEATURES: usize = XSTATE;
const XSTATE_ENABLED_VOLATILE_FEATURES: usize = XSTATE + 0x08;
const XSTATE_SIZE: usize = XSTATE + 0x10;
const XSTATE_CONTROL_FLAGS: usize = XSTATE + 0x14;
const XSTATE_FEATURES: usize = XSTATE + 0x18;
const XSTATE_ALL_FEATURE_SIZE: usize = XSTATE + 0x228;
const XSTATE_ALL_FEATURES: usize = XSTATE + 0x22c;
const XSTATE_ALL_NON_LARGE_FEATURE_SIZE: usize = XSTATE + 0x340;

/// ControlFlags.OptimizedSave (XSAVEOPT); CompactionEnabled stays 0: the
/// CONTEXT_EX areas gala makes are in the standard format.
const XSTATE_CONTROLFLAG_XSAVEOPT: u32 = 1;

const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const PROCESSOR_ARCHITECTURE_AMD64: u16 = 9;
/// ProcessorFeatures[] indices (PF_* in winnt.h) we can always report.
const PF_RDTSC_INSTRUCTION_AVAILABLE: usize = 8;
const PF_XMMI_INSTRUCTIONS_AVAILABLE: usize = 6;
const PF_XMMI64_INSTRUCTIONS_AVAILABLE: usize = 10;
const PF_NX_ENABLED: usize = 12;
const PF_XSAVE_ENABLED: usize = 17;
const PF_AVX_INSTRUCTIONS_AVAILABLE: usize = 39;
const PF_AVX2_INSTRUCTIONS_AVAILABLE: usize = 40;
const PF_AVX512F_INSTRUCTIONS_AVAILABLE: usize = 41;

/// Writes the page's machine, processor features and XState fields.
pub fn write_kuser_processor(page: &SharedPage) {
    page.write(IMAGE_NUMBER_LOW, &IMAGE_FILE_MACHINE_AMD64.to_le_bytes());
    page.write(IMAGE_NUMBER_HIGH, &IMAGE_FILE_MACHINE_AMD64.to_le_bytes());
    page.write(NATIVE_PROCESSOR_ARCHITECTURE, &PROCESSOR_ARCHITECTURE_AMD64.to_le_bytes());
    for pf in [
        PF_RDTSC_INSTRUCTION_AVAILABLE,
        PF_XMMI_INSTRUCTIONS_AVAILABLE,
        PF_XMMI64_INSTRUCTIONS_AVAILABLE,
        PF_NX_ENABLED,
    ] {
        page.write(PROCESSOR_FEATURES + pf, &[1]);
    }
    write_xstate(page);
    page.write(TEST_RET_INSTRUCTION, &[0xc3]); // ret
}

/// The XState configuration (what kernel32's XState functions read) and
/// the processor features that depend on it: the components the platform
/// enables (XCR0), in the standard XSAVE layout.
fn write_xstate(page: &SharedPage) {
    use crate::nt::platform::xstate::{AVX, AVX512, COMPONENTS, LEGACY, size_of};
    let enabled = crate::nt::platform::xstate_enabled();
    let size = size_of(enabled) as u32;
    page.write(XSTATE_ENABLED_FEATURES, &enabled.to_le_bytes());
    page.write(XSTATE_ENABLED_VOLATILE_FEATURES, &enabled.to_le_bytes());
    page.write_u32(XSTATE_SIZE, size);
    page.write_u32(XSTATE_CONTROL_FLAGS, XSTATE_CONTROLFLAG_XSAVEOPT);
    for (i, &(offset, len)) in COMPONENTS.iter().enumerate() {
        if enabled & (1 << i) != 0 {
            page.write_u32(XSTATE_FEATURES + 8 * i, offset as u32);
            page.write_u32(XSTATE_FEATURES + 8 * i + 4, len as u32);
            page.write_u32(XSTATE_ALL_FEATURES + 4 * i, len as u32);
        }
    }
    page.write_u32(XSTATE_ALL_FEATURE_SIZE, size);
    page.write_u32(XSTATE_ALL_NON_LARGE_FEATURE_SIZE, size);
    // What the CPU has and the state that programs can use: AVX2 and
    // AVX-512F need the AVX (and AVX-512) state enabled.
    let leaf7 = crate::nt::platform::cpuid(7, 0).1;
    let features = [
        (PF_XSAVE_ENABLED, enabled != LEGACY),
        (PF_AVX_INSTRUCTIONS_AVAILABLE, enabled & AVX != 0),
        (PF_AVX2_INSTRUCTIONS_AVAILABLE, enabled & AVX != 0 && leaf7 & (1 << 5) != 0),
        (PF_AVX512F_INSTRUCTIONS_AVAILABLE, enabled & AVX512 != 0 && leaf7 & (1 << 16) != 0),
    ];
    for (pf, present) in features {
        page.write(PROCESSOR_FEATURES + pf, &[u8::from(present)]);
    }
}
