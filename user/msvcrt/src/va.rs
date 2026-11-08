//! The C variadic argument list (`va_list`) the `v` functions take: a
//! pointer to the arguments in memory, as Microsoft's x86 and x64 calling
//! conventions lay them out. x64: one 8-byte slot per argument (the
//! caller's home area, then its stack; `variadic` spills the register
//! ones there). x86 (cdecl): the arguments on the stack, each rounded up
//! to 4 bytes (`_INTSIZEOF`), so an int or a pointer takes 4 and a
//! double or an __int64 8, with no alignment between them.

/// A `va_list` (a `char *` in the C headers of both architectures).
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct VaList(*const u8);

/// The size of one argument slot: a pointer.
const SLOT: usize = size_of::<usize>();

impl VaList {
    /// The next argument, a `T` (its size rounded up to whole slots).
    fn take<T: Copy>(&mut self) -> T {
        // SAFETY: the caller passed an argument of this type at this
        // place, as its format says.
        let v = unsafe { self.0.cast::<T>().read_unaligned() };
        let size = size_of::<T>().div_ceil(SLOT) * SLOT;
        // SAFETY: still inside (or just past) the caller's arguments.
        self.0 = unsafe { self.0.add(size) };
        v
    }

    /// An `int`-sized argument (char, short and int arrive promoted).
    pub fn int(&mut self) -> u32 {
        // x64: the low half of the 8-byte slot.
        self.take::<usize>() as u32
    }

    /// A pointer-sized argument (pointers, size_t, ptrdiff_t, `%I`).
    pub fn pointer(&mut self) -> usize {
        self.take::<usize>()
    }

    /// An `__int64` / `long long` argument.
    pub fn int64(&mut self) -> u64 {
        self.take::<u64>()
    }

    /// A `double` (floats arrive promoted).
    pub fn double(&mut self) -> f64 {
        f64::from_bits(self.take::<u64>())
    }

    /// A pointer argument, typed.
    pub fn ptr<T>(&mut self) -> *mut T {
        self.pointer() as *mut T
    }
}
