//! Bitmaps (RTL_BITMAP: SizeOfBitMap bits in a buffer of ULONGs, bit n in
//! bit n % 32 of ULONG n / 32), as the WDK documents the Rtl*Bit*
//! routines; bits past SizeOfBitMap in the buffer's last ULONG are not
//! part of the bitmap (Wine's dlls/ntdll/tests/rtlbitmap.c, run on
//! Windows: a 62-bit map whose last byte is 0xff has a run of 6). Only
//! the clear-run search is exported: Windows' ntdll has no
//! RtlFindNextForwardRunSet (office_api_test), so gala's has none either.

#[repr(C)]
pub struct RtlBitmap {
    size: u32,
    buffer: *mut u32,
}

impl RtlBitmap {
    fn get(&self, bit: u32) -> bool {
        // SAFETY: the caller's buffer holds `size` bits; `bit < size`.
        unsafe { *self.buffer.add((bit / 32) as usize) & (1 << (bit % 32)) != 0 }
    }

    fn put(&self, bit: u32, value: bool) {
        // SAFETY: as `get`; the caller owns the buffer.
        unsafe {
            let w = self.buffer.add((bit / 32) as usize);
            if value {
                *w |= 1 << (bit % 32);
            } else {
                *w &= !(1 << (bit % 32));
            }
        }
    }

    /// The bits set among the first `size`.
    fn count(&self) -> u32 {
        let mut n = 0;
        for i in 0..self.size.div_ceil(32) {
            // SAFETY: as `get`.
            let mut w = unsafe { *self.buffer.add(i as usize) };
            let left = self.size - i * 32;
            if left < 32 {
                w &= (1 << left) - 1;
            }
            n += w.count_ones();
        }
        n
    }

    /// The next run of bits equal to `value` at or after `from`: its start
    /// and length (length 0: none).
    fn run(&self, from: u32, value: bool) -> (u32, u32) {
        let mut i = from;
        while i < self.size && self.get(i) != value {
            i += 1;
        }
        let start = i;
        while i < self.size && self.get(i) == value {
            i += 1;
        }
        (start, i - start)
    }

    /// Sets or clears `count` bits from `start` (within the map).
    fn fill(&self, start: u32, count: u32, value: bool) {
        let end = start.saturating_add(count).min(self.size);
        for bit in start..end {
            self.put(bit, value);
        }
    }
}

/// RtlInitializeBitMap(Header, Buffer, SizeOfBitMap): only the header.
#[unsafe(no_mangle)]
pub extern "system" fn RtlInitializeBitMap(map: *mut RtlBitmap, buffer: *mut u32, size: u32) {
    // SAFETY: the caller's header.
    unsafe { *map = RtlBitmap { size, buffer } };
}

fn map<'a>(map: *const RtlBitmap) -> &'a RtlBitmap {
    // SAFETY: the caller's initialized header.
    unsafe { &*map }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlSetBits(m: *const RtlBitmap, start: u32, count: u32) {
    map(m).fill(start, count, true);
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlClearBits(m: *const RtlBitmap, start: u32, count: u32) {
    map(m).fill(start, count, false);
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlSetAllBits(m: *const RtlBitmap) {
    let m = map(m);
    m.fill(0, m.size, true);
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlClearAllBits(m: *const RtlBitmap) {
    let m = map(m);
    m.fill(0, m.size, false);
}

/// RtlCheckBit(Header, BitPosition): 1 when the bit is set.
#[unsafe(no_mangle)]
pub extern "system" fn RtlCheckBit(m: *const RtlBitmap, bit: u32) -> u8 {
    let m = map(m);
    u8::from(bit < m.size && m.get(bit))
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlNumberOfSetBits(m: *const RtlBitmap) -> u32 {
    map(m).count()
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlNumberOfClearBits(m: *const RtlBitmap) -> u32 {
    let m = map(m);
    m.size - m.count()
}

/// The `RtlFindNextForwardRun*` rule: the length of the next run at or
/// after `from` (0 when there is none or the arguments are not valid),
/// its start in `*pos`.
fn next_run(m: *const RtlBitmap, from: u32, pos: *mut u32, value: bool) -> u32 {
    if m.is_null() || pos.is_null() {
        return 0;
    }
    let m = map(m);
    if from >= m.size {
        return 0;
    }
    let (start, length) = m.run(from, value);
    if length != 0 {
        // SAFETY: the caller's output.
        unsafe { *pos = start };
    }
    length
}

/// RtlFindNextForwardRunClear(Header, FromIndex, StartingRunIndex).
#[unsafe(no_mangle)]
pub extern "system" fn RtlFindNextForwardRunClear(
    m: *const RtlBitmap,
    from: u32,
    pos: *mut u32,
) -> u32 {
    next_run(m, from, pos, false)
}
