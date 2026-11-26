//! What the AHCI and NVMe drivers need of the machine, as two small
//! traits, so that their logic runs on the host against a fake
//! controller (`tests`): a controller's registers, memory the controller
//! reads and writes (DMA), and a clock for the specifications' timeouts.
//! The kernel's implementation is `KernelPlatform` (`arch::mmio::Mmio`,
//! `arch::dma::DmaRegion`, the TSC's clock).

use crate::arch::dma::DmaRegion;
use crate::arch::mmio::Mmio;

/// Memory the controller may read and write (physically contiguous).
pub trait Mem {
    /// Its physical (bus) address.
    fn phys(&self) -> u64;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read32(&self, offset: usize) -> u32;
    fn write32(&mut self, offset: usize, value: u32);
    fn copy_in(&mut self, offset: usize, data: &[u8]);
    fn copy_out(&self, offset: usize, out: &mut [u8]);
    /// Zeroes `len` bytes from `offset`.
    fn zero(&mut self, offset: usize, len: usize) {
        let mut at = offset;
        while at < offset + len {
            self.write32(at, 0);
            at += 4;
        }
    }
}

/// A controller: its register window (32-bit accesses; a 64-bit register
/// is two, low half first, which AHCI and NVMe both allow), its memory,
/// and the time.
pub trait Platform {
    type Mem: Mem;
    fn read32(&self, offset: usize) -> u32;
    fn write32(&mut self, offset: usize, value: u32);
    fn read64(&self, offset: usize) -> u64 {
        u64::from(self.read32(offset)) | u64::from(self.read32(offset + 4)) << 32
    }
    fn write64(&mut self, offset: usize, value: u64) {
        self.write32(offset, value as u32);
        self.write32(offset + 4, (value >> 32) as u32);
    }
    /// Zeroed memory of at least `bytes` the controller can reach.
    fn alloc(&mut self, bytes: usize) -> Option<Self::Mem>;
    /// A monotonic clock in nanoseconds.
    fn now_ns(&self) -> u64;
}

pub const MS: u64 = 1_000_000;
pub const SECOND: u64 = 1_000 * MS;

/// Spins until `cond` holds or `timeout_ns` passed; whether it held. No
/// scheduling: it runs at boot, in a recovery and from a panic.
pub fn wait_for<P: Platform>(p: &P, timeout_ns: u64, mut cond: impl FnMut(&P) -> bool) -> bool {
    let start = p.now_ns();
    loop {
        if cond(p) {
            return true;
        }
        if p.now_ns().saturating_sub(start) > timeout_ns {
            return cond(p);
        }
        core::hint::spin_loop();
    }
}

/// Spins for `ns` (a specification's minimum hold time).
pub fn delay<P: Platform>(p: &P, ns: u64) {
    wait_for(p, ns, |_| false);
}

/// Text of up to `N` bytes (a model name, a serial number) for the log:
/// printable ASCII, trailing spaces and NULs dropped.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Text<N> {
    pub fn new(raw: &[u8]) -> Self {
        let mut bytes = [0u8; N];
        let mut len = 0;
        for (i, &b) in raw.iter().take(N).enumerate() {
            bytes[i] = if (0x20..0x7f).contains(&b) { b } else { b' ' };
            if b != b' ' && b != 0 {
                len = i + 1;
            }
        }
        // Leading spaces too (ATA strings are often right-aligned).
        let start = bytes[..len].iter().position(|&b| b != b' ').unwrap_or(len);
        bytes.copy_within(start..len, 0);
        Self { bytes, len: len - start }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }
}

impl<const N: usize> core::fmt::Display for Text<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<const N: usize> core::fmt::Debug for Text<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl Mem for DmaRegion {
    fn phys(&self) -> u64 {
        DmaRegion::phys(self)
    }
    fn len(&self) -> usize {
        DmaRegion::len(self)
    }
    fn read32(&self, offset: usize) -> u32 {
        self.read::<u32>(offset)
    }
    fn write32(&mut self, offset: usize, value: u32) {
        self.write::<u32>(offset, value);
    }
    fn copy_in(&mut self, offset: usize, data: &[u8]) {
        DmaRegion::copy_in(self, offset, data);
    }
    fn copy_out(&self, offset: usize, out: &mut [u8]) {
        DmaRegion::copy_out(self, offset, out);
    }
}

/// The kernel's controller: a mapped register window and DMA regions.
pub struct KernelPlatform {
    regs: Mmio,
}

impl KernelPlatform {
    pub fn new(regs: Mmio) -> Self {
        Self { regs }
    }

    /// The register window's physical address (for the log).
    pub fn regs_phys(&self) -> u64 {
        self.regs.phys()
    }
}

impl Platform for KernelPlatform {
    type Mem = DmaRegion;
    fn read32(&self, offset: usize) -> u32 {
        self.regs.read32(offset)
    }
    fn write32(&mut self, offset: usize, value: u32) {
        // What the driver wrote to memory is seen by the controller
        // before the register write that tells it to look (a doorbell).
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.regs.write32(offset, value);
    }
    fn alloc(&mut self, bytes: usize) -> Option<DmaRegion> {
        DmaRegion::new(bytes)
    }
    fn now_ns(&self) -> u64 {
        crate::time::raw_monotonic_ns()
    }
}

#[cfg(test)]
pub mod fake {
    //! Host-side guest memory for the fake controllers: regions at
    //! made-up physical addresses, shared between the driver's `Mem`s and
    //! the controller model that reads and writes them.
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::vec::Vec;

    #[derive(Default)]
    pub struct Phys {
        regions: BTreeMap<u64, Vec<u8>>,
        next: u64,
    }

    pub type Shared = Rc<RefCell<Phys>>;

    impl Phys {
        pub fn shared() -> Shared {
            Rc::new(RefCell::new(Phys { regions: BTreeMap::new(), next: 0x10_0000 }))
        }

        pub fn alloc(&mut self, bytes: usize) -> u64 {
            let len = bytes.div_ceil(4096) * 4096;
            let at = self.next;
            self.regions.insert(at, std::vec![0; len]);
            self.next += len as u64 + 4096;
            at
        }

        fn at(&mut self, addr: u64, len: usize) -> &mut [u8] {
            let (base, bytes) = self
                .regions
                .range_mut(..=addr)
                .next_back()
                .unwrap_or_else(|| panic!("no memory at {addr:#x}"));
            let o = (addr - base) as usize;
            assert!(o + len <= bytes.len(), "DMA at {addr:#x}+{len:#x} past its region");
            &mut bytes[o..o + len]
        }

        pub fn read(&mut self, addr: u64, out: &mut [u8]) {
            out.copy_from_slice(self.at(addr, out.len()));
        }

        pub fn write(&mut self, addr: u64, data: &[u8]) {
            self.at(addr, data.len()).copy_from_slice(data);
        }

        pub fn read32(&mut self, addr: u64) -> u32 {
            let mut b = [0u8; 4];
            self.read(addr, &mut b);
            u32::from_le_bytes(b)
        }

        pub fn write32(&mut self, addr: u64, v: u32) {
            self.write(addr, &v.to_le_bytes());
        }

        pub fn read64(&mut self, addr: u64) -> u64 {
            u64::from(self.read32(addr)) | u64::from(self.read32(addr + 4)) << 32
        }
    }

    pub struct FakeMem {
        pub phys: Shared,
        pub at: u64,
        pub len: usize,
    }

    impl super::Mem for FakeMem {
        fn phys(&self) -> u64 {
            self.at
        }
        fn len(&self) -> usize {
            self.len
        }
        fn read32(&self, offset: usize) -> u32 {
            assert!(offset + 4 <= self.len);
            self.phys.borrow_mut().read32(self.at + offset as u64)
        }
        fn write32(&mut self, offset: usize, value: u32) {
            assert!(offset + 4 <= self.len);
            self.phys.borrow_mut().write32(self.at + offset as u64, value);
        }
        fn copy_in(&mut self, offset: usize, data: &[u8]) {
            assert!(offset + data.len() <= self.len);
            self.phys.borrow_mut().write(self.at + offset as u64, data);
        }
        fn copy_out(&self, offset: usize, out: &mut [u8]) {
            assert!(offset + out.len() <= self.len);
            self.phys.borrow_mut().read(self.at + offset as u64, out);
        }
    }

    /// A clock that moves 50 microseconds at each look, so a timeout
    /// passes after a bounded number of polls.
    #[derive(Default)]
    pub struct Clock(Cell<u64>);

    impl Clock {
        pub fn now(&self) -> u64 {
            let t = self.0.get() + 50_000;
            self.0.set(t);
            t
        }
    }

    #[test]
    fn text_trims_and_replaces() {
        let t = super::Text::<12>::new(b"  QEMU DISK \0\0");
        assert_eq!(t.as_str(), "QEMU DISK");
        assert_eq!(super::Text::<4>::new(b"a\x01b").as_str(), "a b");
        assert_eq!(super::Text::<4>::new(b"    ").as_str(), "");
    }
}
