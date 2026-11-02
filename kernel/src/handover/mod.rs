//! Kernel handover (ROADMAP R9, ARCHITECTURE "Kernel handover"): the
//! running kernel writes a record of what the next kernel needs (instead
//! of the bootloader's answers), stops the machine at a safe point and
//! enters a fresh kernel image, which starts from that record.
//!
//! Milestone (a): the same build, no programs running. Carried: the memory
//! map (with what is in use), the HHDM and the new image's place, the
//! framebuffer, the command line, the boot modules, the clock, the parked
//! CPUs, the mounted volume's log position, the kernel file, and kept pages
//! (a marker page that survives every handover). The machine side (image
//! loading, page tables, trampoline, parking CPUs, the jump) is
//! `arch::handover`.
//!
//! Milestone (b): programs that are idle (every thread blocked in a wait)
//! come across and go on: `carry` collects and rebuilds them, `programs`
//! has their record formats. A program can ask for a handover
//! (GalaRequestHandover, with `handover=`).
//!
//! Milestone (c): busy programs and programs on gala's DLLs: threads are
//! brought to safe points (held on their way back to user mode, with
//! their full user context, or blocked in a call that can be made
//! again), DLL views are laid out again and checked by digest, pipes,
//! keyed events, tokens, APC queues and security descriptors are carried.

pub mod carry;
pub mod elf;
pub mod programs;
pub mod record;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::handover as machine;
use crate::boot::BootInfo;
use crate::cmdline::Cmdline;
use crate::mm::PAGE_SIZE;
use crate::mm::heap::{AllocError, TryVec};
use crate::mm::memmap::{self, MemoryRegion, RegionKind};
use record::{Range, purpose, tag};

/// The kernel's link address and the most virtual space its image may take.
const KERNEL_VIRT: u64 = 0xffff_ffff_8000_0000;
const KERNEL_MAX_SPAN: u64 = 1 << 30;

/// Handovers done before this kernel started (0 on a cold boot).
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

#[derive(Debug)]
pub enum HandoverError {
    NoKernelFile,
    Elf(elf::ElfError),
    OutOfMemory,
    /// CPU indices with a hole (a CPU that did not come up).
    CpuHoles,
    Machine(machine::HandoverError),
    RecordTooLarge,
    /// The programs cannot be carried (the reason); they run on.
    Refused(carry::Refusal),
}

impl From<AllocError> for HandoverError {
    fn from(_: AllocError) -> Self {
        HandoverError::OutOfMemory
    }
}

impl From<machine::HandoverError> for HandoverError {
    fn from(e: machine::HandoverError) -> Self {
        HandoverError::Machine(e)
    }
}

/// The marker page: "GALAMARK", a value set at the first boot, the
/// handovers it has been through, and a CRC-32 of those 24 bytes.
const MARKER_MAGIC: [u8; 8] = *b"GALAMARK";

fn marker_bytes(value: u64, handovers: u64) -> [u8; 28] {
    let mut m = [0u8; 28];
    m[0..8].copy_from_slice(&MARKER_MAGIC);
    m[8..16].copy_from_slice(&value.to_le_bytes());
    m[16..24].copy_from_slice(&handovers.to_le_bytes());
    let crc = crate::fs::log::crc32(0, &m[..24]);
    m[24..28].copy_from_slice(&crc.to_le_bytes());
    m
}

/// (value, handovers) of a marker page's first bytes, if intact.
fn parse_marker(m: &[u8; 28]) -> Option<(u64, u64)> {
    let crc = u32::from_le_bytes(m[24..28].try_into().ok()?);
    (m[0..8] == MARKER_MAGIC && crc == crate::fs::log::crc32(0, &m[..24])).then(|| {
        (
            u64::from_le_bytes(m[8..16].try_into().unwrap_or([0; 8])),
            u64::from_le_bytes(m[16..24].try_into().unwrap_or([0; 8])),
        )
    })
}

/// The kept ranges the previous kernel listed.
fn received_kept(info: &BootInfo) -> impl Iterator<Item = Range> + '_ {
    let section = info.handover.as_ref().and_then(|r| r.record.section(tag::KEPT));
    section.into_iter().flat_map(|s| KeptList::from_iter(record::read_ranges(&s)))
}

/// A small fixed list (no allocation while reading the record).
struct KeptList {
    items: [Option<Range>; 16],
}

impl FromIterator<Range> for KeptList {
    fn from_iter<I: IntoIterator<Item = Range>>(iter: I) -> Self {
        let mut items = [None; 16];
        for (slot, r) in items.iter_mut().zip(iter) {
            *slot = Some(r);
        }
        KeptList { items }
    }
}

impl IntoIterator for KeptList {
    type Item = Range;
    type IntoIter = core::iter::Flatten<core::array::IntoIter<Option<Range>, 16>>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().flatten()
    }
}

/// Early in boot (console up): says where this kernel came from.
pub fn report(info: &BootInfo) {
    let Some(r) = &info.handover else { return };
    GENERATION.store(r.generation, Ordering::Relaxed);
    crate::info!(
        "handover: generation {} (from generation {}), record version {}, {} bytes at {:#x}",
        r.generation,
        r.generation.saturating_sub(1),
        r.version,
        r.record_len,
        r.record_phys
    );
    for section in r.record.sections().flatten() {
        if !record::KNOWN.contains(&section.tag) {
            crate::warn!("handover: skipped section {} (unknown to this kernel)", section.tag);
        }
    }
}

/// Before the clock starts: the TSC and timer rates the first kernel
/// calibrated (calibrating again gives other values, under an emulator
/// very different ones, and the tick count would then drift from the TSC:
/// the deadlines of carried waits are ticks).
pub fn carried_rates(info: &BootInfo, tsc_hz: &mut u64, timer_hz: &mut u64) {
    let Some(r) = &info.handover else { return };
    let Some(mut s) = r.record.section(tag::CLOCK) else { return };
    let clock = record::Clock::read(&mut s);
    if clock.tsc_hz != 0 {
        *tsc_hz = clock.tsc_hz;
    }
    if clock.timer_hz != 0 {
        *timer_hz = clock.timer_hz;
    }
}

/// After the clock started: checks it went on from the previous kernel's.
pub fn resume_clock(info: &BootInfo) {
    let Some(r) = &info.handover else { return };
    let Some(mut s) = r.record.section(tag::CLOCK) else {
        crate::warn!("handover: no clock in the record; time starts again at 0");
        return;
    };
    let clock = record::Clock::read(&mut s);
    crate::time::resume(&clock);
    let now = crate::time::raw_monotonic_ns();
    if now < clock.monotonic_ns {
        panic!("handover: the clock went back from {} ns to {now} ns", clock.monotonic_ns);
    }
    crate::info!(
        "handover: clock monotonic: {} ms at the record, {} ms now (+{} us)",
        clock.monotonic_ns / 1_000_000,
        now / 1_000_000,
        (now - clock.monotonic_ns) / 1000
    );
}

/// After the CPUs started: says how many came back.
pub fn report_cpus(info: &BootInfo, online: u32) {
    if let Some(r) = &info.handover {
        crate::info!(
            "handover: generation {}: {online} of {} CPUs online again",
            r.generation,
            info.processors.len()
        );
    }
}

/// After the volume is mounted (`persist`): checks it is where the
/// previous kernel left it.
pub fn check_volume(info: &BootInfo) {
    let Some(r) = &info.handover else { return };
    let Some(mut s) = r.record.section(tag::VOLUME) else { return };
    let before = record::Volume::read(&mut s);
    match crate::nt::persist::position() {
        Some(now) if now == before => crate::info!(
            "handover: volume continues: log generation {}, area {}, {} bytes (as handed over)",
            now.generation,
            now.area,
            now.tail
        ),
        Some(now) => crate::error!("handover: volume moved: {before:?} handed over, {now:?} found"),
        None => {
            crate::error!("handover: a volume was handed over ({before:?}) but none is mounted")
        }
    }
}

/// Checks the marker page the previous kernels kept (its handover count
/// must be this kernel's generation).
pub fn check_marker(info: &BootInfo) {
    let Some(r) = &info.handover else { return };
    let Some(page) = received_kept(info).find(|k| k.what == u64::from(purpose::MARKER)) else {
        return;
    };
    let mut m = [0u8; 28];
    machine::OwnedPages::kept(page.base, page.len).read(0, &mut m);
    match parse_marker(&m) {
        Some((value, handovers)) if handovers == r.generation => crate::info!(
            "handover: marker {value:#x} survived {handovers} handover(s) at {:#x}",
            page.base
        ),
        other => panic!("handover: the marker page at {:#x} is not intact: {other:?}", page.base),
    }
}

/// `handover=<n>`: whether this kernel should hand over (its generation is
/// below n).
pub fn wanted(cmdline: &Cmdline) -> bool {
    cmdline.value("handover").and_then(|v| v.parse::<u64>().ok()).is_some_and(|n| generation() < n)
}

/// Hands the machine over to a fresh copy of the kernel (the kernel file,
/// or the boot module `handover_image=`), with the programs that run
/// (milestone (b): once each of their threads is blocked in a wait).
/// Returns only on failure, before the point of no return; the programs
/// then go on in this kernel.
pub fn go(info: &BootInfo, cmdline: &Cmdline) -> HandoverError {
    let programs = match carry::collect() {
        Ok(p) => p,
        Err(why) => return HandoverError::Refused(why),
    };
    if let Some(p) = &programs {
        crate::info!(
            "handover: carrying {} process(es), {} thread(s) ({} stopped in user mode), {} \
             object(s), {} user frame(s)",
            p.process_count(),
            p.thread_count(),
            p.user_mode_count(),
            p.object_count(),
            p.frame_count()
        );
    }
    let e = match prepare_and_go(info, cmdline, programs.as_ref()) {
        Err(e) => e,
        Ok(never) => match never {},
    };
    if programs.is_some() {
        carry::release();
    }
    e
}

/// After the files are there (`nt::init`, boot modules, `persist`): the
/// programs the previous kernel handed over resume.
pub fn resume_programs(info: &BootInfo) {
    if let Some(r) = &info.handover {
        carry::restore(&r.record);
    }
}

enum Never {}

fn prepare_and_go(
    info: &BootInfo,
    cmdline: &Cmdline,
    programs: Option<&carry::Programs>,
) -> Result<Never, HandoverError> {
    let hhdm = info.hhdm_offset;
    let file = match cmdline.value("handover_image") {
        Some(name) => info.modules.find(name).map(|m| m.data),
        None => info.kernel_file,
    };
    let file = file.ok_or(HandoverError::NoKernelFile)?;
    let image = elf::parse(file, KERNEL_VIRT, KERNEL_MAX_SPAN).map_err(HandoverError::Elf)?;
    let next = generation() + 1;

    // The marker: made at the first handover, counted at each.
    let (mut marker, value) =
        match received_kept(info).find(|k| k.what == u64::from(purpose::MARKER)) {
            Some(page) => {
                let page = machine::OwnedPages::kept(page.base, page.len);
                let mut m = [0u8; 28];
                page.read(0, &mut m);
                let (value, _) = parse_marker(&m).unwrap_or((0, 0));
                (page, value)
            }
            None => {
                let page = machine::OwnedPages::allocate(1).ok_or(HandoverError::OutOfMemory)?;
                let value = crate::arch::cpu::rdtsc() ^ 0x6761_6c61_6d61_726b;
                crate::info!("handover: marker {value:#x} set at {:#x}", page.phys());
                (page, value)
            }
        };
    marker.write(0, &marker_bytes(value, next));

    // The disk: what is pending written, so the log position is final.
    crate::nt::persist::sync();
    let volume = crate::nt::persist::position();

    // All on the boot CPU from here.
    crate::sched::set_affinity(crate::sched::current(), 1);
    let cpus: Vec<(u32, u32)> = {
        let mut v = Vec::new();
        for c in crate::arch::smp::online_cpus() {
            v.try_push(c)?;
        }
        v
    };
    if cpus.iter().enumerate().any(|(i, c)| c.0 != i as u32)
        || crate::arch::percpu::cpu_index() != 0
    {
        return Err(HandoverError::CpuHoles);
    }
    let mut lapic_ids = Vec::new();
    for c in &cpus {
        lapic_ids.try_push(c.1)?;
    }

    // The memory map for the next kernel: what the previous handover kept
    // for this kernel only (scaffold, record, image) is free again.
    let mut map: Vec<MemoryRegion> = Vec::new();
    for r in info.memory_map.iter() {
        map.try_push(r)?;
    }
    for k in received_kept(info) {
        if matches!(
            k.what as u32,
            purpose::SCAFFOLD | purpose::RECORD | purpose::KERNEL_IMAGE | purpose::BITMAP
        ) {
            memmap::carve(&mut map, k.base, k.len, RegionKind::Usable)?;
        }
    }
    memmap::carve(&mut map, marker.phys(), marker.len(), RegionKind::ExecutableAndModules)?;
    // Room for the next kernel's frame bitmap, from free frames (usable
    // memory holds the programs' pages, where `boot::take_frame_bitmap`
    // would otherwise put it).
    let bitmap_end = map
        .iter()
        .filter(|r| matches!(r.kind, RegionKind::Usable | RegionKind::BootloaderReclaimable))
        .map(|r| r.end())
        .max()
        .unwrap_or(0);
    let bitmap_bytes = crate::mm::frame::FrameAllocator::words_for(bitmap_end) as u64 * 8;
    let bitmap = machine::OwnedPages::allocate(bitmap_bytes.div_ceil(PAGE_SIZE))
        .ok_or(HandoverError::OutOfMemory)?;
    memmap::carve(&mut map, bitmap.phys(), bitmap.len(), RegionKind::BootloaderReclaimable)?;

    let prepared = machine::prepare(&image, map.iter().copied(), &lapic_ids)?;
    memmap::carve(&mut map, prepared.area.0, prepared.area.1, RegionKind::BootloaderReclaimable)?;
    memmap::carve(&mut map, prepared.image.0, prepared.image.1, RegionKind::ExecutableAndModules)?;

    // The record: sized with a first encoding, then written for good into
    // its own frames (which the map then shows in use).
    let file_phys = (file.as_ptr() as u64).wrapping_sub(hhdm);
    let state = State {
        info,
        cmdline,
        image: &image,
        prepared: &prepared,
        volume,
        marker: (marker.phys(), marker.len()),
        file: (file_phys, file.len() as u64),
        next,
        bsp_lapic_id: cpus.first().map_or(0, |c| c.1),
        bitmap: (bitmap.phys(), bitmap.len()),
        programs,
    };
    let estimate = state.encode(&map, (0, 0))?.len() as u64;
    let pages = (estimate + 2 * PAGE_SIZE).div_ceil(PAGE_SIZE);
    let mut record_pages =
        machine::OwnedPages::allocate(pages).ok_or(HandoverError::OutOfMemory)?;
    let record_at = (record_pages.phys(), record_pages.len());
    memmap::carve(&mut map, record_at.0, record_at.1, RegionKind::BootloaderReclaimable)?;
    let bytes = state.encode(&map, record_at)?;
    if !record_pages.write(0, &bytes) {
        return Err(HandoverError::RecordTooLarge);
    }

    crate::info!(
        "handover: generation {} -> {next}: record {} bytes at {:#x}, kernel image {} KiB at \
         {:#x} (entry {:#x}), scaffold at {:#x}; parking {} CPU(s)",
        generation(),
        bytes.len(),
        record_at.0,
        prepared.image.1 / 1024,
        prepared.image.0,
        image.entry,
        prepared.area.0,
        cpus.len() - 1
    );
    let others = &cpus[1..];
    drop(map);
    drop(bytes);
    Err(machine::quiesce_and_jump(&prepared, record_at.0, others).into())
}

/// What goes into the record.
struct State<'a> {
    info: &'a BootInfo,
    cmdline: &'a Cmdline<'a>,
    image: &'a elf::KernelImage<'a>,
    prepared: &'a machine::Prepared,
    volume: Option<record::Volume>,
    marker: (u64, u64),
    file: (u64, u64),
    next: u64,
    bsp_lapic_id: u32,
    bitmap: (u64, u64),
    programs: Option<&'a carry::Programs>,
}

impl State<'_> {
    fn encode(&self, map: &[MemoryRegion], record_at: (u64, u64)) -> Result<Vec<u8>, AllocError> {
        let info = self.info;
        let mut w = record::Writer::new(self.next)?;
        record::write_ranges(
            &mut w,
            tag::MEMORY_MAP,
            map.iter().map(|r| Range { base: r.base, len: r.len, what: r.kind.to_limine() }),
        )?;
        record::Platform {
            hhdm_offset: info.hhdm_offset,
            kernel_phys: self.prepared.image.0,
            kernel_virt: self.image.base,
            kernel_len: self.prepared.image.1,
            firmware: info.firmware.map_or(0, |f| f.to_limine() + 1),
            boot_unix_time: info.boot_unix_time.map_or(0, |t| t.max(0) as u64 + 1),
        }
        .write(&mut w)?;
        if let Some(fb) = info.framebuffer_facts {
            fb.write(&mut w)?;
        }
        w.section(tag::CMDLINE, 1, 0)?;
        w.bytes(self.cmdline.as_str().as_bytes())?;
        record::write_modules(
            &mut w,
            info.modules.iter().map(|m| record::ModuleEntry {
                phys: m.phys(info.hhdm_offset),
                len: m.data.len() as u64,
                path: m.path,
                cmdline: m.cmdline,
            }),
        )?;
        crate::time::snapshot().write(&mut w)?;
        record::Cpus {
            bsp_lapic_id: self.bsp_lapic_id,
            count: self.prepared.parked.1,
            parked_phys: self.prepared.parked.0,
        }
        .write(&mut w)?;
        if let Some(v) = self.volume {
            v.write(&mut w)?;
        }
        let kept = [
            (self.prepared.area, purpose::SCAFFOLD),
            (record_at, purpose::RECORD),
            (self.prepared.image, purpose::KERNEL_IMAGE),
            (self.marker, purpose::MARKER),
            (self.bitmap, purpose::BITMAP),
        ];
        record::write_ranges(
            &mut w,
            tag::KEPT,
            kept.iter().map(|&((base, len), p)| Range { base, len, what: u64::from(p) }),
        )?;
        record::write_ranges(
            &mut w,
            tag::KERNEL_FILE,
            core::iter::once(Range { base: self.file.0, len: self.file.1, what: 0 }),
        )?;
        if let Some(p) = self.programs {
            p.write(&mut w)?;
        }
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_round_trip() {
        let m = marker_bytes(0x1234, 2);
        assert_eq!(parse_marker(&m), Some((0x1234, 2)));
        let mut bad = m;
        bad[9] ^= 1;
        assert_eq!(parse_marker(&bad), None);
    }
}
