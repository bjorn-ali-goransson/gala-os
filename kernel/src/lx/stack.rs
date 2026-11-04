//! The initial process stack of a Linux x86_64 program, built as bytes:
//! what the exec path copies to the top of the new stack before the first
//! instruction runs with rsp at [`InitialStack::sp`].
//!
//! Sources (CLEAN_ROOM.md "Linux"): the System V AMD64 ABI ("Process
//! Initialization": argc, argv, envp, the auxiliary vector, rsp 16-byte
//! aligned at entry) and real Linux 6.18's layout as user/lx/auxv_test
//! records it (its expected.txt), which this module reproduces byte for
//! byte where Linux is deterministic (the tests below replay auxv_test's
//! transcript against a stack built here):
//!
//! ```text
//! end (page aligned)   8 zero bytes (the end marker)
//!                      AT_EXECFN's string (the path given to execve)
//!                      envp's strings, in order, each NUL-terminated
//!                      argv's strings, in order        <- argv[0]
//!                      a gap (random on Linux; `gap` here)
//!                      AT_PLATFORM's string ("x86_64"), ending 16-aligned
//!                      AT_RANDOM's 16 bytes, right below it
//!                      padding: the least that makes sp 16-aligned
//!                      auxv: (type, value) pairs, AT_NULL last
//!                      envp[0..envc], NULL
//!                      argv[0..argc], NULL
//! sp (16-aligned)      argc
//! ```
//!
//! The auxv's order is Linux's (auxv_test's "auxv order" line); entries
//! Linux has and gala does not yet (AT_SYSINFO_EHDR without a vDSO,
//! AT_MINSIGSTKSZ, AT_HWCAP2, AT_RSEQ_*) are optional or left out:
//! programs look entries up by type.

use alloc::vec::Vec;
use core::fmt;

/// Auxiliary vector types (System V AMD64 ABI; the Linux-specific ones as
/// musl's elf.h and auxv_test give them).
pub mod at {
    pub const NULL: u64 = 0;
    pub const PHDR: u64 = 3;
    pub const PHENT: u64 = 4;
    pub const PHNUM: u64 = 5;
    pub const PAGESZ: u64 = 6;
    pub const BASE: u64 = 7;
    pub const FLAGS: u64 = 8;
    pub const ENTRY: u64 = 9;
    pub const UID: u64 = 11;
    pub const EUID: u64 = 12;
    pub const GID: u64 = 13;
    pub const EGID: u64 = 14;
    pub const PLATFORM: u64 = 15;
    pub const HWCAP: u64 = 16;
    pub const CLKTCK: u64 = 17;
    pub const SECURE: u64 = 23;
    pub const RANDOM: u64 = 25;
    pub const HWCAP2: u64 = 26;
    pub const EXECFN: u64 = 31;
    pub const SYSINFO_EHDR: u64 = 33;
    pub const MINSIGSTKSZ: u64 = 51;
}

/// AT_PLATFORM's string on x86_64 (auxv_test).
pub const PLATFORM: &[u8] = b"x86_64";
/// AT_CLKTCK: clock ticks per second as times(2) counts them (auxv_test).
pub const CLKTCK: u64 = 100;

/// The auxiliary vector's values; [`build`] adds AT_RANDOM, AT_EXECFN and
/// AT_PLATFORM, which point into the stack.
#[derive(Clone, Copy, Debug, Default)]
pub struct Aux {
    /// AT_PHDR, AT_PHENT, AT_PHNUM, AT_ENTRY: the program's (its
    /// `elf::LoadPlan`'s), even when an interpreter runs first.
    pub phdr: u64,
    pub phent: u64,
    pub phnum: u64,
    pub entry: u64,
    /// AT_BASE: the interpreter's load address; 0 for a static program.
    pub base: u64,
    pub pagesz: u64,
    pub flags: u64,
    pub uid: u64,
    pub euid: u64,
    pub gid: u64,
    pub egid: u64,
    /// AT_SECURE: set-uid/set-gid or a capability gain: libc then ignores
    /// LD_* and the like.
    pub secure: bool,
    /// AT_HWCAP: CPUID leaf 1's EDX on x86_64 (auxv_test).
    pub hwcap: u64,
    pub clktck: u64,
    pub hwcap2: Option<u64>,
    /// The vDSO's ELF header, once gala has one.
    pub sysinfo_ehdr: Option<u64>,
    pub minsigstksz: Option<u64>,
}

/// What goes on the stack.
#[derive(Clone, Copy, Debug)]
pub struct StackParams<'a> {
    pub argv: &'a [&'a [u8]],
    pub envp: &'a [&'a [u8]],
    /// The path execve was given (AT_EXECFN).
    pub execfn: &'a [u8],
    /// AT_PLATFORM's string ([`PLATFORM`]).
    pub platform: &'a [u8],
    /// AT_RANDOM's bytes (libc's stack-protector canary and pointer
    /// guard seed): from the kernel's random source.
    pub random: [u8; 16],
    pub aux: Aux,
    /// Bytes left free below the strings before AT_PLATFORM (Linux leaves
    /// a random gap there; 0 until gala randomizes).
    pub gap: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackError {
    /// `end` is not page-aligned, or so low the stack would wrap.
    BadEnd {
        end: u64,
    },
    /// A string with a NUL inside (it would read as two).
    NulInString {
        which: &'static str,
        index: usize,
    },
    /// The stack needs more than `max` bytes (execve's E2BIG).
    TooBig {
        needed: u64,
        max: u64,
    },
    OutOfMemory,
}

impl fmt::Display for StackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::BadEnd { end } => write!(f, "stack end {end:#x} not page aligned or too low"),
            Self::NulInString { which, index } => write!(f, "{which}[{index}] has a NUL inside"),
            Self::TooBig { needed, max } => {
                write!(f, "initial stack needs {needed} bytes, more than {max}")
            }
            Self::OutOfMemory => write!(f, "out of memory"),
        }
    }
}

/// The built stack: `bytes` belong at `sp..end`.
#[derive(Debug)]
pub struct InitialStack {
    pub bytes: Vec<u8>,
    pub sp: u64,
    pub end: u64,
    pub argv: u64,
    pub envp: u64,
    pub auxv: u64,
    pub execfn: u64,
    pub platform: u64,
    pub random: u64,
}

impl InitialStack {
    /// The u64 at `addr`, if it lies in the stack.
    pub fn u64_at(&self, addr: u64) -> Option<u64> {
        let at = usize::try_from(addr.checked_sub(self.sp)?).ok()?;
        let b = self.bytes.get(at..at.checked_add(8)?)?;
        let mut w = [0u8; 8];
        w.copy_from_slice(b);
        Some(u64::from_le_bytes(w))
    }

    /// The NUL-terminated string at `addr` (without the NUL).
    pub fn str_at(&self, addr: u64) -> Option<&[u8]> {
        let at = usize::try_from(addr.checked_sub(self.sp)?).ok()?;
        let rest = self.bytes.get(at..)?;
        rest.iter().position(|&b| b == 0).map(|n| &rest[..n])
    }
}

fn strings_size(list: &[&[u8]], which: &'static str) -> Result<u64, StackError> {
    let mut size = 0u64;
    for (index, s) in list.iter().enumerate() {
        if s.contains(&0) {
            return Err(StackError::NulInString { which, index });
        }
        size = size.saturating_add(s.len() as u64 + 1);
    }
    Ok(size)
}

/// Builds the stack that ends at `end` (page aligned; the stack's top)
/// and uses at most `max` bytes.
pub fn build(end: u64, max: u64, p: &StackParams<'_>) -> Result<InitialStack, StackError> {
    if !end.is_multiple_of(4096) || end < max {
        return Err(StackError::BadEnd { end });
    }
    let args = strings_size(p.argv, "argv")?;
    let envs = strings_size(p.envp, "envp")?;
    let execfn_size = strings_size(&[p.execfn], "execfn")?;
    let platform_size = strings_size(&[p.platform], "platform")?;
    let mut aux = [(0u64, 0u64); 24];
    let mut naux = 0;
    let mut add = |t: u64, v: u64| {
        aux[naux] = (t, v);
        naux += 1;
    };
    let a = &p.aux;
    if let Some(v) = a.sysinfo_ehdr {
        add(at::SYSINFO_EHDR, v);
    }
    if let Some(v) = a.minsigstksz {
        add(at::MINSIGSTKSZ, v);
    }
    add(at::HWCAP, a.hwcap);
    add(at::PAGESZ, a.pagesz);
    add(at::CLKTCK, a.clktck);
    add(at::PHDR, a.phdr);
    add(at::PHENT, a.phent);
    add(at::PHNUM, a.phnum);
    add(at::BASE, a.base);
    add(at::FLAGS, a.flags);
    add(at::ENTRY, a.entry);
    add(at::UID, a.uid);
    add(at::EUID, a.euid);
    add(at::GID, a.gid);
    add(at::EGID, a.egid);
    add(at::SECURE, u64::from(a.secure));
    // AT_RANDOM, AT_EXECFN and AT_PLATFORM point into the stack: filled below.
    add(at::RANDOM, 0);
    if let Some(v) = a.hwcap2 {
        add(at::HWCAP2, v);
    }
    add(at::EXECFN, 0);
    add(at::PLATFORM, 0);
    add(at::NULL, 0);

    // Addresses, top down; all sizes are bounded by `max` before use.
    let too_big = |needed: u64| StackError::TooBig { needed, max };
    let strings = 8u64.saturating_add(execfn_size).saturating_add(envs).saturating_add(args);
    let above_platform = strings.saturating_add(p.gap).saturating_add(15) & !15;
    let below = platform_size + 16;
    let table = 8 * (1 + p.argv.len() as u64 + 1 + p.envp.len() as u64 + 1 + 2 * naux as u64);
    let needed =
        (above_platform.saturating_add(below).saturating_add(table).saturating_add(15)) & !15;
    if needed > max {
        return Err(too_big(needed));
    }
    let execfn = end - 8 - execfn_size;
    let env_strings = execfn - envs;
    let arg_strings = env_strings - args;
    let platform_end = (arg_strings - p.gap) & !15;
    let platform = platform_end - platform_size;
    let random = platform - 16;
    let sp = (random - table) & !15;
    let size = end - sp;

    let mut bytes = Vec::new();
    let len = usize::try_from(size).map_err(|_| too_big(size))?;
    bytes.try_reserve_exact(len).map_err(|_| StackError::OutOfMemory)?;
    bytes.resize(len, 0);
    // Every address below is in sp..end (the sizes were checked above).
    let put = |bytes: &mut [u8], addr: u64, data: &[u8]| {
        let at = (addr - sp) as usize;
        bytes[at..at + data.len()].copy_from_slice(data);
    };
    put(&mut bytes, execfn, p.execfn);
    put(&mut bytes, platform, p.platform);
    put(&mut bytes, random, &p.random);

    let mut words = sp;
    let mut push = |bytes: &mut [u8], v: u64| {
        put(bytes, words, &v.to_le_bytes());
        words += 8;
    };
    push(&mut bytes, p.argv.len() as u64);
    let mut s = arg_strings;
    let mut strings = |bytes: &mut [u8], push: &mut dyn FnMut(&mut [u8], u64), list: &[&[u8]]| {
        for item in list {
            put(bytes, s, item);
            push(bytes, s);
            s += item.len() as u64 + 1;
        }
        push(bytes, 0);
    };
    strings(&mut bytes, &mut push, p.argv);
    strings(&mut bytes, &mut push, p.envp);
    for &(t, v) in &aux[..naux] {
        let v = match t {
            at::RANDOM => random,
            at::EXECFN => execfn,
            at::PLATFORM => platform,
            _ => v,
        };
        push(&mut bytes, t);
        push(&mut bytes, v);
    }
    let argv = sp + 8;
    let envp = argv + 8 * (p.argv.len() as u64 + 1);
    let auxv = envp + 8 * (p.envp.len() as u64 + 1);
    Ok(InitialStack { bytes, sp, end, argv, envp, auxv, execfn, platform, random })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lx::elf::{DEFAULT_DYN_BASE, ElfImage};
    use alloc::string::String;
    use core::fmt::Write;

    /// What user/lx's runner (`cargo xtask lx-expect`) gives auxv_test.
    const ARGV: &[&[u8]] = &[b"./auxv_test", b"first", b"", b"third arg"];
    const ENVP: &[&[u8]] = &[b"A=1", b"EMPTY=", b"LONGER_NAME=some value"];
    const EXPECTED: &str = include_str!("../../../user/lx/auxv_test/expected.txt");
    const AUXV_TEST: &[u8] = include_bytes!("../../../user/lx/bin/auxv_test");
    const AUXV_TEST_PIE: &[u8] = include_bytes!("../../../user/lx/bin/auxv_test-pie");
    const END: u64 = 0x7ffd_8000_0000;
    const HWCAP: u64 = 0x178b_fbff;

    fn params(file: &[u8]) -> (StackParams<'static>, ElfImage) {
        let image = ElfImage::parse(file).unwrap();
        let plan = image.plan(image.load_bias(DEFAULT_DYN_BASE).unwrap()).unwrap();
        let aux = Aux {
            phdr: plan.phdr,
            phent: plan.phent,
            phnum: plan.phnum,
            entry: plan.entry,
            pagesz: 4096,
            hwcap: HWCAP,
            clktck: CLKTCK,
            uid: 1000,
            euid: 1000,
            gid: 100,
            egid: 100,
            ..Aux::default()
        };
        let p = StackParams {
            argv: ARGV,
            envp: ENVP,
            execfn: b"./auxv_test",
            platform: PLATFORM,
            random: *b"0123456789abcdef",
            aux,
            gap: 0,
        };
        (p, image)
    }

    /// auxv_test's transcript, computed from a stack built here the way
    /// the program computes it from the one Linux gave it.
    fn transcript(st: &InitialStack, image: &ElfImage, bias: u64) -> String {
        let mut o = String::new();
        let yes = |b: bool| if b { "yes" } else { "no" };
        let word = |a: u64| st.u64_at(a).unwrap();
        let text = |a: u64| String::from_utf8_lossy(st.str_at(a).unwrap()).into_owned();
        let argc = word(st.sp);
        let mut at = [0u64; 64];
        o.push_str("auxv order (the types checked):");
        let mut a = st.auxv;
        loop {
            let (t, v) = (word(a), word(a + 8));
            if t < 64 {
                at[t as usize] = v;
            }
            let name = match t {
                at::PHDR => "PHDR",
                at::PHENT => "PHENT",
                at::PHNUM => "PHNUM",
                at::PAGESZ => "PAGESZ",
                at::BASE => "BASE",
                at::FLAGS => "FLAGS",
                at::ENTRY => "ENTRY",
                at::UID => "UID",
                at::EUID => "EUID",
                at::GID => "GID",
                at::EGID => "EGID",
                at::PLATFORM => "PLATFORM",
                at::HWCAP => "HWCAP",
                at::CLKTCK => "CLKTCK",
                at::SECURE => "SECURE",
                at::RANDOM => "RANDOM",
                at::EXECFN => "EXECFN",
                at::NULL => "NULL",
                _ => "",
            };
            if !name.is_empty() {
                write!(o, " {name}").unwrap();
            }
            a += 16;
            if t == at::NULL {
                break;
            }
        }
        let auxv_end = a;
        writeln!(o).unwrap();
        writeln!(o, "argc = {argc}").unwrap();
        writeln!(o, "*sp = {argc}").unwrap();
        writeln!(o, "sp % 16 = {}", st.sp % 16).unwrap();
        let argv: Vec<u64> = (0..argc).map(|i| word(st.argv + 8 * i)).collect();
        for (i, &s) in argv.iter().enumerate() {
            writeln!(o, "argv[{i}] = \"{}\"", text(s)).unwrap();
        }
        writeln!(o, "argv[argc] NULL = {}", yes(word(st.argv + 8 * argc) == 0)).unwrap();
        let mut envp = Vec::new();
        while word(st.envp + 8 * envp.len() as u64) != 0 {
            envp.push(word(st.envp + 8 * envp.len() as u64));
        }
        for (i, &s) in envp.iter().enumerate() {
            writeln!(o, "envp[{i}] = \"{}\"", text(s)).unwrap();
        }
        writeln!(o, "envp right after argv's NULL = {}", yes(st.envp == st.argv + 8 * (argc + 1)))
            .unwrap();
        let execfn = at[at::EXECFN as usize];
        let end = execfn + st.str_at(execfn).unwrap().len() as u64 + 1 + 8;
        writeln!(o, "end % 4096 = {}", end % 4096).unwrap();
        writeln!(o, "the end marker = {}", word(end - 8)).unwrap();
        writeln!(
            o,
            "AT_EXECFN is argv[0]'s text = {}",
            yes(st.str_at(execfn) == st.str_at(argv[0]))
        )
        .unwrap();
        for (i, &s) in argv.iter().enumerate() {
            writeln!(o, "argv[{i}] at end-{}", end - s).unwrap();
        }
        for (i, &s) in envp.iter().enumerate() {
            writeln!(o, "envp[{i}] at end-{}", end - s).unwrap();
        }
        writeln!(o, "AT_EXECFN at end-{}", end - execfn).unwrap();
        o.push_str("string area =");
        for b in &st.bytes[(argv[0] - st.sp) as usize..(end - st.sp) as usize] {
            write!(o, " {b:02x}").unwrap();
        }
        writeln!(o).unwrap();
        let platform = at[at::PLATFORM as usize];
        let random = at[at::RANDOM as usize];
        let platform_end = platform + st.str_at(platform).unwrap().len() as u64 + 1;
        writeln!(o, "AT_PLATFORM[0] = \"{}\"", text(platform)).unwrap();
        writeln!(o, "AT_PLATFORM's end 16-byte aligned = {}", yes(platform_end.is_multiple_of(16)))
            .unwrap();
        writeln!(o, "AT_PLATFORM below the strings = {}", yes(platform_end <= argv[0])).unwrap();
        writeln!(o, "AT_RANDOM right below AT_PLATFORM = {}", yes(random + 16 == platform))
            .unwrap();
        let r = &st.bytes[(random - st.sp) as usize..(random - st.sp) as usize + 16];
        writeln!(o, "AT_RANDOM's bytes not all zero = {}", yes(r.iter().any(|&b| b != 0))).unwrap();
        let table = auxv_end - st.sp;
        writeln!(
            o,
            "auxv's end padded the least below AT_RANDOM = {}",
            yes(auxv_end <= random && st.sp == (random - table) & !15)
        )
        .unwrap();
        // The ELF header relations, with the header where the plan puts
        // the segment holding file offset 0.
        let ehdr = image.segments.iter().find(|s| s.offset == 0).unwrap().vaddr + bias;
        writeln!(o, "AT_PAGESZ = {}", at[at::PAGESZ as usize]).unwrap();
        writeln!(o, "AT_PHENT = {}", at[at::PHENT as usize]).unwrap();
        writeln!(
            o,
            "AT_PHNUM is e_phnum = {}",
            yes(at[at::PHNUM as usize] == u64::from(image.phnum))
        )
        .unwrap();
        writeln!(
            o,
            "AT_PHDR is the headers' address = {}",
            yes(at[at::PHDR as usize] == ehdr + image.phoff)
        )
        .unwrap();
        writeln!(
            o,
            "AT_ENTRY is e_entry + the load bias = {}",
            yes(at[at::ENTRY as usize] == image.entry + bias)
        )
        .unwrap();
        let dyn_ok = if image.kind == crate::lx::elf::Kind::Dyn { bias != 0 } else { bias == 0 };
        writeln!(o, "ET_DYN loaded at a non-zero bias = {}", yes(dyn_ok)).unwrap();
        writeln!(o, "AT_BASE = {}", at[at::BASE as usize]).unwrap();
        writeln!(o, "AT_FLAGS = {}", at[at::FLAGS as usize]).unwrap();
        // The process's ids and CPUID are the ones the stack was built with.
        writeln!(o, "AT_UID is getuid() = {}", yes(at[at::UID as usize] == 1000)).unwrap();
        writeln!(o, "AT_EUID is geteuid() = {}", yes(at[at::EUID as usize] == 1000)).unwrap();
        writeln!(o, "AT_GID is getgid() = {}", yes(at[at::GID as usize] == 100)).unwrap();
        writeln!(o, "AT_EGID is getegid() = {}", yes(at[at::EGID as usize] == 100)).unwrap();
        writeln!(o, "AT_SECURE = {}", at[at::SECURE as usize]).unwrap();
        writeln!(o, "AT_CLKTCK = {}", at[at::CLKTCK as usize]).unwrap();
        writeln!(o, "AT_HWCAP is CPUID(1).EDX = {}", yes(at[at::HWCAP as usize] == HWCAP)).unwrap();
        writeln!(o, "auxv_test: ok").unwrap();
        writeln!(o, "-- exit status 0").unwrap();
        o
    }

    /// The stack built here, read the way auxv_test reads Linux's, gives
    /// Linux's transcript (user/lx/auxv_test/expected.txt) line for line,
    /// the string area byte for byte; for the static and the static-pie
    /// build, with and without a gap.
    #[test]
    fn matches_what_linux_gives_auxv_test() {
        for file in [AUXV_TEST, AUXV_TEST_PIE] {
            for gap in [0, 1, 100, 4095] {
                let (mut p, image) = params(file);
                p.gap = gap;
                let st = build(END, 128 * 1024, &p).unwrap();
                let bias = image.load_bias(DEFAULT_DYN_BASE).unwrap();
                let got = transcript(&st, &image, bias);
                for (n, (want, got)) in EXPECTED.lines().zip(got.lines()).enumerate() {
                    assert_eq!(got, want, "line {} (gap {gap})", n + 1);
                }
                assert_eq!(got.lines().count(), EXPECTED.lines().count());
            }
        }
    }

    #[test]
    fn layout_details() {
        let (mut p, _) = params(AUXV_TEST);
        // Optional entries go where Linux puts them.
        p.aux.sysinfo_ehdr = Some(0x7fff_f7ff_d000);
        p.aux.minsigstksz = Some(0xe30);
        p.aux.hwcap2 = Some(2);
        let st = build(END, 1 << 20, &p).unwrap();
        let mut types = Vec::new();
        let mut a = st.auxv;
        loop {
            let t = st.u64_at(a).unwrap();
            types.push(t);
            a += 16;
            if t == at::NULL {
                break;
            }
        }
        assert_eq!(
            types,
            [33, 51, 16, 6, 17, 3, 4, 5, 7, 8, 9, 11, 12, 13, 14, 23, 25, 26, 31, 15, 0]
        );
        assert_eq!(st.sp % 16, 0);
        assert_eq!(st.bytes.len() as u64, END - st.sp);
        assert_eq!(st.str_at(st.platform), Some(PLATFORM));
        assert_eq!(&st.bytes[(st.random - st.sp) as usize..][..16], b"0123456789abcdef");
        assert_eq!(st.u64_at(END - 8), Some(0));
        assert_eq!(st.u64_at(END), None);
    }

    #[test]
    fn errors() {
        let (p, _) = params(AUXV_TEST);
        assert_eq!(build(END + 8, 1 << 20, &p).unwrap_err(), StackError::BadEnd { end: END + 8 });
        assert_eq!(build(0x1000, 1 << 20, &p).unwrap_err(), StackError::BadEnd { end: 0x1000 });
        assert!(matches!(build(END, 256, &p), Err(StackError::TooBig { max: 256, .. })));
        let nul: &[&[u8]] = &[b"a", b"b\0c"];
        let q = StackParams { envp: nul, ..p };
        assert_eq!(
            build(END, 1 << 20, &q).unwrap_err(),
            StackError::NulInString { which: "envp", index: 1 }
        );
        // A huge gap is refused, not wrapped.
        let q = StackParams { gap: u64::MAX - 10, ..p };
        assert!(matches!(build(END, 1 << 20, &q), Err(StackError::TooBig { .. })));
        // Exactly enough is enough.
        let st = build(END, 1 << 20, &p).unwrap();
        let exact = st.bytes.len() as u64;
        assert!(build(END, exact, &p).is_ok());
        assert!(build(END, exact - 16, &p).is_err());
        // No arguments, no environment.
        let q = StackParams { argv: &[], envp: &[], ..p };
        let st = build(END, 1 << 20, &q).unwrap();
        assert_eq!(st.u64_at(st.sp), Some(0));
        assert_eq!(st.envp, st.argv + 8);
        assert_eq!(st.auxv, st.envp + 8);
    }
}
