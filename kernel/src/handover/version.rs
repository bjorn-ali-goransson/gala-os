//! Versions of the handover record and their negotiation (ROADMAP R9
//! milestone (d), ARCHITECTURE "Versions and negotiation"): what this
//! kernel writes and reads of each section, the note that says so in its
//! image, and the check a running kernel makes against the next kernel's
//! note before the point of no return.
//!
//! Every section kind has, in this kernel, the version it writes (also the
//! newest it reads), the oldest it reads (older ones are converted on
//! reading: fields the older writer did not have take their defaults), and
//! the oldest reader version that may take what it writes without losing
//! state (`must_read`, written into each section's header: a field a
//! reader may ignore keeps it, a field whose loss changes what a program
//! sees raises it to the section's new version).
//!
//! The kernel image carries the same facts in an ELF note (PT_NOTE, name
//! "GALA", type [`NOTE_TYPE`]), so a running kernel can read them from the
//! file it is about to hand over to:
//!
//! ```text
//! desc  0  note format (u32, 1)     4  record framing: oldest read (u32)
//!       8  newest framing (u32)    12  image layout version (u32)
//!      16  extended state it restores (u64, XCR0 bits)
//!      24  section kinds (u32)     28  0 (u32)
//!      32  build name (32 bytes, UTF-8, zero padded)
//!      64  per kind: tag (u32), oldest version read (u16), newest (u16)
//! ```
//!
//! A kernel built before notes existed (milestone (c)) has none: it reads
//! [`BEFORE_NOTES`]. The signed manifest of R8 ("Chain of trust") will
//! check the image before any of this ([`trusted`]).

use core::fmt;

use super::elf::KernelImage;
use super::record::{self, tag};

/// The `handover_next` build: a stand-in for the next release, with a
/// changed state format (PROCESSES version 4, the WRITER section, the
/// CLOCK section's in-memory structure laid out differently). The boot
/// tests hand over between it and the default build both ways.
pub const NEXT: bool = cfg!(feature = "handover_next");

/// This build's name (the note, the WRITER section, the logs).
pub const BUILD: &str = if NEXT { "gala-next" } else { "gala" };

/// The version of the way images are laid out (`nt::section::ImageLayout`:
/// headers and sections at their RVAs, the zeroed tails, relocation at an
/// alternative base). Programs' image pages are mapped from the next
/// kernel's layout, so a kernel whose layout version differs is refused
/// before the point of no return (the digest checks after it would lose
/// the programs). Raise it with any change to how an image's bytes are
/// laid out.
pub const IMAGE_LAYOUT: u32 = crate::nt::section::LAYOUT_VERSION;

/// The extended processor state this kernel can restore, as XCR0 bits:
/// x87, SSE, AVX and the AVX-512 components (`arch::xstate::SUPPORTED`;
/// which of them it enables depends on the CPU, chosen at boot). A
/// running kernel carries its threads' state under its own XCR0 (THREADS'
/// extended-state blob) and refuses a next kernel whose note lacks any of
/// its bits; the next kernel refuses the programs if its own XCR0 differs.
pub const XSTATE: u64 = crate::arch::xstate::SUPPORTED;

/// What this kernel does with one kind of section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    pub tag: u32,
    pub name: &'static str,
    /// The version it writes, and the newest it reads (0: not known to
    /// this build).
    pub writes: u16,
    /// The oldest version it reads (converted with defaults).
    pub reads_from: u16,
    /// The oldest reader version that may take the section it writes.
    pub must_read: u16,
    /// REQUIRED: a reader that does not know the tag must refuse.
    pub required: bool,
    /// Written only when programs are carried.
    pub programs: bool,
    /// Written only when the programs hold such state (sockets): checked
    /// on the record as written (`check_record`), not up front by
    /// `agree`, so programs without it still go to a kernel that
    /// predates the section.
    pub when_present: bool,
}

const fn kind(tag: u32, name: &'static str, writes: u16, must_read: u16, programs: bool) -> Kind {
    Kind {
        tag,
        name,
        writes,
        reads_from: 0,
        must_read,
        required: programs,
        programs,
        when_present: false,
    }
}

/// A programs' section written only when there is such state.
const fn present(tag: u32, name: &'static str, writes: u16, must_read: u16) -> Kind {
    Kind { when_present: true, ..kind(tag, name, writes, must_read, true) }
}

/// Every kind of section, as this build writes and reads it. Programs'
/// sections are REQUIRED (a kernel that does not know one would lose
/// programs). Version history (each change is a new version and a line
/// here and in ARCHITECTURE "Versions and negotiation"):
/// - CLOCK 2: the APIC timer rate (optional to understand).
/// - PROCESSES 2: the executable's digest, the primary token (a reader
///   that dropped the token would run the program as the boot user:
///   must_read 2). 3 (RS B1): the process's session (a version-2 reader
///   takes its token's TokenSessionId, the same unless the token was moved
///   to another session after the process started: must_read stays 2).
///   4 (`handover_next` only): the handovers the process came through,
///   marked must-understand to stand in for such a change.
/// - THREADS 2: how a thread stood (USER, START: a version-1 reader would
///   make a USER thread's "call" again), suspend counts, APCs, the
///   impersonation token (must_read 2). 3: the extended-state blob after
///   the APCs (empty while gala uses FXSAVE; a writer that fills it marks
///   the section must_read 3, `programs::threads_must_read`).
/// - TIMERS 2: the timer's APC (must_read 2: losing it loses a call).
/// - NET 1, SOCKETS 1 (milestone (e)): the TCP stack and the programs'
///   sockets, written only when a program holds a socket. SOCKETS 2: UDP
///   sockets and datagram requests (must_read 2 only when the record holds
///   a UDP socket, `sockets::sockets_must_read`; version 1 is read with
///   the new fields absent). SOCKETS 3: an endpoint's sending side
///   (must_read 3 only while a shutdown's FIN waits for pending sends).
/// - STOPPED 1 (2026-11-03): when the programs stopped (optional; GalaHandoverPause).
pub const KINDS: [Kind; 36] = [
    kind(tag::MEMORY_MAP, "MEMORY_MAP", 1, 0, false),
    kind(tag::PLATFORM, "PLATFORM", 1, 0, false),
    kind(tag::FRAMEBUFFER, "FRAMEBUFFER", 1, 0, false),
    kind(tag::CMDLINE, "CMDLINE", 1, 0, false),
    kind(tag::MODULES, "MODULES", 1, 0, false),
    kind(tag::CLOCK, "CLOCK", 2, 0, false),
    kind(tag::CPUS, "CPUS", 1, 0, false),
    kind(tag::VOLUME, "VOLUME", 1, 0, false),
    kind(tag::KEPT, "KEPT", 1, 0, false),
    kind(tag::KERNEL_FILE, "KERNEL_FILE", 1, 0, false),
    kind(tag::USER_FRAMES, "USER_FRAMES", 1, 0, true),
    kind(tag::PROCESSES, "PROCESSES", if NEXT { 4 } else { 3 }, if NEXT { 4 } else { 2 }, true),
    kind(tag::ADDRESS_SPACES, "ADDRESS_SPACES", 1, 0, true),
    kind(tag::THREADS, "THREADS", 3, 2, true),
    kind(tag::EVENTS, "EVENTS", 1, 0, true),
    kind(tag::SEMAPHORES, "SEMAPHORES", 1, 0, true),
    kind(tag::MUTANTS, "MUTANTS", 1, 0, true),
    kind(tag::TIMERS, "TIMERS", 2, 2, true),
    kind(tag::SECTIONS, "SECTIONS", 1, 0, true),
    kind(tag::FILES, "FILES", 1, 0, true),
    kind(tag::IO_COMPLETIONS, "IO_COMPLETIONS", 1, 0, true),
    kind(tag::EXITED_THREADS, "EXITED_THREADS", 1, 0, true),
    kind(tag::PROCESS_OBJECTS, "PROCESS_OBJECTS", 1, 0, true),
    kind(tag::CONSOLES, "CONSOLES", 1, 0, true),
    kind(tag::HANDLES, "HANDLES", 1, 0, true),
    kind(tag::IMAGE_SECTIONS, "IMAGE_SECTIONS", 1, 0, true),
    kind(tag::FILE_SECTIONS, "FILE_SECTIONS", 1, 0, true),
    kind(tag::PIPES, "PIPES", 1, 0, true),
    kind(tag::PIPE_ENDS, "PIPE_ENDS", 1, 0, true),
    kind(tag::KEYED_EVENTS, "KEYED_EVENTS", 1, 0, true),
    kind(tag::SECURITY, "SECURITY", 1, 0, true),
    kind(tag::TOKENS, "TOKENS", 1, 0, true),
    // The writer's name (optional; `handover_next` only, for now).
    kind(tag::WRITER, "WRITER", if NEXT { 1 } else { 0 }, 0, false),
    present(tag::NET, "NET", 1, 0),
    present(tag::SOCKETS, "SOCKETS", 3, 0),
    // When the programs stopped (optional: a reader that skips it reports
    // the pause from the record on).
    kind(tag::STOPPED, "STOPPED", 1, 0, false),
];

/// This build's facts about section `tag` (None: it does not know it).
pub fn kind_of(tag: u32) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.tag == tag && k.writes != 0)
}

/// Whether this build knows section `tag`.
pub fn known(tag: u32) -> bool {
    kind_of(tag).is_some()
}

/// The version of section `tag` this build writes (0: none).
pub fn writes(tag: u32) -> u16 {
    kind_of(tag).map_or(0, |k| k.writes)
}

/// A section's name for the logs.
pub fn name(tag: u32) -> &'static str {
    KINDS.iter().find(|k| k.tag == tag).map_or("an unknown section", |k| k.name)
}

/// The note's name field ("GALA" and its terminator, padded to 4).
const NOTE_NAME: [u8; 8] = *b"GALA\0\0\0\0";
/// The note's type: what a kernel reads of a handover record.
pub const NOTE_TYPE: u32 = 0x4741_0001;
/// The note's own format version.
pub const NOTE_FORMAT: u32 = 1;
const DESC_HEADER: usize = 64;
const NAME_LEN: usize = 32;

const fn count_known(kinds: &[Kind]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < kinds.len() {
        if kinds[i].writes != 0 {
            n += 1;
        }
        i += 1;
    }
    n
}

const COUNT: usize = count_known(&KINDS);
const DESC_LEN: usize = DESC_HEADER + 8 * COUNT;
const NOTE_LEN: usize = 12 + NOTE_NAME.len() + DESC_LEN;

const fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    let mut i = 0;
    while i < bytes.len() {
        buf[at + i] = bytes[i];
        i += 1;
    }
}

/// The note's description: the facts `KINDS` and the constants give.
const fn desc<const N: usize>(
    framing: (u32, u32),
    layout: u32,
    xstate: u64,
    build: &str,
    kinds: &[Kind],
) -> [u8; N] {
    let mut d = [0u8; N];
    put(&mut d, 0, &NOTE_FORMAT.to_le_bytes());
    put(&mut d, 4, &framing.0.to_le_bytes());
    put(&mut d, 8, &framing.1.to_le_bytes());
    put(&mut d, 12, &layout.to_le_bytes());
    put(&mut d, 16, &xstate.to_le_bytes());
    put(&mut d, 24, &(count_known(kinds) as u32).to_le_bytes());
    put(&mut d, 32, build.as_bytes());
    let mut at = DESC_HEADER;
    let mut i = 0;
    while i < kinds.len() {
        let k = kinds[i];
        if k.writes != 0 {
            put(&mut d, at, &k.tag.to_le_bytes());
            put(&mut d, at + 4, &k.reads_from.to_le_bytes());
            put(&mut d, at + 6, &k.writes.to_le_bytes());
            at += 8;
        }
        i += 1;
    }
    d
}

const fn note_bytes() -> [u8; NOTE_LEN] {
    let mut n = [0u8; NOTE_LEN];
    put(&mut n, 0, &5u32.to_le_bytes()); // "GALA\0"
    put(&mut n, 4, &(DESC_LEN as u32).to_le_bytes());
    put(&mut n, 8, &NOTE_TYPE.to_le_bytes());
    put(&mut n, 12, &NOTE_NAME);
    let d: [u8; DESC_LEN] =
        desc((record::MIN_VERSION, record::VERSION), IMAGE_LAYOUT, XSTATE, BUILD, &KINDS);
    put(&mut n, 12 + NOTE_NAME.len(), &d);
    n
}

/// The note (header, name, description), 8-aligned: `boot::HANDOVER_NOTE`
/// puts it into the image's `.gala_note` section, a PT_NOTE segment of its
/// own (kernel/linker.ld).
#[repr(C, align(8))]
pub struct Note(pub [u8; NOTE_LEN]);

/// The note's bytes.
pub const NOTE_CONTENTS: Note = Note(note_bytes());

/// This kernel's note, as the image carries it.
fn note() -> &'static Note {
    &crate::boot::HANDOVER_NOTE
}

/// What a kernel reads of a handover record (from its note).
#[derive(Clone, Copy, Debug)]
pub struct Reads<'a> {
    /// The record framing versions it reads (oldest, newest).
    pub framing: (u32, u32),
    pub image_layout: u32,
    pub xstate: u64,
    build: &'a [u8],
    /// (tag, oldest, newest) records of 8 bytes.
    kinds: &'a [u8],
}

impl<'a> Reads<'a> {
    /// Reads a note's description; None if it is not one this kernel
    /// understands (another note format, damaged).
    pub fn parse(desc: &'a [u8]) -> Option<Self> {
        let u32_at =
            |at: usize| desc.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        if u32_at(0)? != NOTE_FORMAT {
            return None;
        }
        let xstate = u64::from(u32_at(16)?) | u64::from(u32_at(20)?) << 32;
        let count = u32_at(24)? as usize;
        let name = desc.get(32..32 + NAME_LEN)?;
        let len = name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        let kinds = desc.get(DESC_HEADER..DESC_HEADER.checked_add(count.checked_mul(8)?)?)?;
        Some(Reads {
            framing: (u32_at(4)?, u32_at(8)?),
            image_layout: u32_at(12)?,
            xstate,
            build: &name[..len],
            kinds,
        })
    }

    /// This kernel's own (its note).
    pub fn mine() -> Reads<'static> {
        let desc = &note().0[12 + NOTE_NAME.len()..];
        // The note is built from the constants above; a host test checks
        // that it parses.
        Reads::parse(desc).unwrap_or(BEFORE_NOTES)
    }

    /// The build's name.
    pub fn build(&self) -> &'a str {
        core::str::from_utf8(self.build).unwrap_or("?")
    }

    /// The versions of section `tag` it reads (oldest, newest).
    pub fn section(&self, tag: u32) -> Option<(u16, u16)> {
        self.kinds.chunks_exact(8).find_map(|c| {
            let t = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            (t == tag).then(|| (u16::from_le_bytes([c[4], c[5]]), u16::from_le_bytes([c[6], c[7]])))
        })
    }

    /// How many section kinds it knows.
    pub fn kinds(&self) -> usize {
        self.kinds.len() / 8
    }
}

/// The facts of the milestone (c) kernel, the last one without a note:
/// record framing 1, sections 1 to 32, PROCESSES, THREADS, TIMERS and
/// CLOCK at version 2, the rest at 1, every older version read, image
/// layout 1, FXSAVE only.
const BEFORE_NOTES_KINDS: [Kind; 32] = {
    let mut k = [kind(0, "", 0, 0, false); 32];
    let mut i = 0;
    while i < 32 {
        let t = i as u32 + 1;
        let v = match t {
            tag::CLOCK | tag::PROCESSES | tag::THREADS | tag::TIMERS => 2,
            _ => 1,
        };
        k[i] = kind(t, "", v, 0, false);
        i += 1;
    }
    k
};
const BEFORE_NOTES_DESC: [u8; DESC_HEADER + 8 * 32] =
    desc((1, 1), 1, 0b11, "gala before R9 (d)", &BEFORE_NOTES_KINDS);
pub const BEFORE_NOTES: Reads<'static> = Reads {
    framing: (1, 1),
    image_layout: 1,
    xstate: 0b11,
    build: b"gala before R9 (d)",
    kinds: {
        let (_, kinds) = BEFORE_NOTES_DESC.split_at(DESC_HEADER);
        kinds
    },
};

/// The facts of the kernel in `image` (its note; [`BEFORE_NOTES`] if it
/// has none). Err if it has a note this kernel cannot read.
pub fn of_image<'a>(image: &KernelImage<'a>) -> Result<Reads<'a>, Mismatch> {
    match image.note(&NOTE_NAME[..5], NOTE_TYPE) {
        Some(desc) => Reads::parse(desc).ok_or(Mismatch::Note),
        None => Ok(BEFORE_NOTES),
    }
}

/// R8 ("Chain of trust", step 4): the image must be listed, with its
/// size and SHA-256, in the release's signed manifest that this kernel
/// verified at boot (`trust`) before the machine is handed to it. An
/// unsigned build (no release key built in) accepts every image.
pub fn trusted(file: &[u8]) -> Result<(), Mismatch> {
    crate::trust::trusted(file).map_err(Mismatch::Untrusted)
}

/// A name kept by value (the target's build, for a refusal's message).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Name([u8; NAME_LEN], u8);

impl Name {
    fn of(s: &str) -> Self {
        let mut n = [0u8; NAME_LEN];
        let len = s.len().min(NAME_LEN);
        n[..len].copy_from_slice(&s.as_bytes()[..len]);
        Name(n, len as u8)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(core::str::from_utf8(&self.0[..usize::from(self.1)]).unwrap_or("?"))
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

/// Why the next kernel cannot take this kernel's record (the handover is
/// refused before the point of no return; the programs go on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mismatch {
    /// The image has a GALA note this kernel cannot read.
    Note,
    /// It does not read the record framing this kernel writes.
    Framing { target: Name, writes: u32, reads: (u32, u32) },
    /// It lays images out differently: the programs' image pages would
    /// not be the same bytes.
    ImageLayout { target: Name, mine: u32, theirs: u32 },
    /// A thread's extended state has components it does not restore.
    ExtendedState { target: Name, needs: u64, restores: u64 },
    /// A REQUIRED section it does not know.
    Unknown { target: Name, tag: u32 },
    /// A REQUIRED section it knows only in versions older than the
    /// section needs.
    TooNew { target: Name, tag: u32, version: u16, must_read: u16, reads: u16 },
    /// A REQUIRED section older than it reads.
    TooOld { target: Name, tag: u32, version: u16, reads_from: u16 },
    /// The image is not in the signed manifest (R8).
    Untrusted(crate::trust::Untrusted),
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Mismatch::Note => f.write_str("the next kernel's version note cannot be read"),
            Mismatch::Framing { target, writes, reads: (lo, hi) } => write!(
                f,
                "the next kernel ({target}) reads record framing {lo} to {hi}; this kernel writes \
                 {writes}"
            ),
            Mismatch::ImageLayout { target, mine, theirs } => write!(
                f,
                "the next kernel ({target}) lays images out by version {theirs}, this kernel by \
                 {mine}: the programs' image pages would differ"
            ),
            Mismatch::ExtendedState { target, needs, restores } => write!(
                f,
                "the next kernel ({target}) restores extended state {restores:#x}; the threads \
                 need {needs:#x}"
            ),
            Mismatch::Unknown { target, tag } => write!(
                f,
                "the next kernel ({target}) does not know section {} ({tag}), which it must read",
                name(tag)
            ),
            Mismatch::TooNew { target, tag, version, must_read, reads } => write!(
                f,
                "the next kernel ({target}) reads {} up to version {reads}; this kernel writes \
                 version {version}, which needs a reader of version {must_read}",
                name(tag)
            ),
            Mismatch::TooOld { target, tag, version, reads_from } => write!(
                f,
                "the next kernel ({target}) reads {} from version {reads_from}; this kernel \
                 writes version {version}",
                name(tag)
            ),
            Mismatch::Untrusted(why) => write!(f, "the next kernel's image is not trusted: {why}"),
        }
    }
}

/// Whether `target` takes a section (tag, version, must_read, REQUIRED)
/// as this kernel writes it. An optional section it cannot take is
/// skipped there (nothing is lost that it would have used).
pub fn check_section(
    target: &Reads<'_>,
    tag: u32,
    version: u16,
    must_read: u16,
    required: bool,
) -> Result<(), Mismatch> {
    if !required {
        return Ok(());
    }
    let t = Name::of(target.build());
    match target.section(tag) {
        None => Err(Mismatch::Unknown { target: t, tag }),
        Some((_, reads)) if must_read > reads => {
            Err(Mismatch::TooNew { target: t, tag, version, must_read, reads })
        }
        Some((reads_from, _)) if version < reads_from => {
            Err(Mismatch::TooOld { target: t, tag, version, reads_from })
        }
        Some(_) => Ok(()),
    }
}

/// Before anything is stopped: whether `target` reads what this kernel
/// writes (its framing, its sections; with programs, their sections and
/// the image layout).
pub fn agree(target: &Reads<'_>, programs: bool) -> Result<(), Mismatch> {
    let t = Name::of(target.build());
    if !(target.framing.0..=target.framing.1).contains(&record::VERSION) {
        return Err(Mismatch::Framing {
            target: t,
            writes: record::VERSION,
            reads: target.framing,
        });
    }
    if programs && target.image_layout != IMAGE_LAYOUT {
        return Err(Mismatch::ImageLayout {
            target: t,
            mine: IMAGE_LAYOUT,
            theirs: target.image_layout,
        });
    }
    // Sections written only when there is such state (sockets) are
    // checked on the record as written (`check_record`).
    for k in KINDS.iter().filter(|k| k.writes != 0 && !k.when_present && (programs || !k.programs))
    {
        check_section(target, k.tag, k.writes, k.must_read, k.required)?;
    }
    Ok(())
}

/// The threads' extended state (`needs`, XCR0 bits) against what
/// `target` restores.
pub fn agree_xstate(target: &Reads<'_>, needs: u64) -> Result<(), Mismatch> {
    if needs & !target.xstate != 0 {
        return Err(Mismatch::ExtendedState {
            target: Name::of(target.build()),
            needs,
            restores: target.xstate,
        });
    }
    Ok(())
}

/// The record as written, section by section, against `target` (the same
/// rules as `agree`, on the versions and flags actually written).
pub fn check_record(bytes: &[u8], target: &Reads<'_>) -> Result<(), Mismatch> {
    let t = Name::of(target.build());
    let framing = bytes.get(8..12).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if !(target.framing.0..=target.framing.1).contains(&framing) {
        return Err(Mismatch::Framing { target: t, writes: framing, reads: target.framing });
    }
    let Ok(rec) = record::parse_framing(bytes) else {
        return Err(Mismatch::Framing { target: t, writes: framing, reads: target.framing });
    };
    for s in rec.sections().flatten() {
        check_section(target, s.tag, s.version, s.must_read, s.flags & record::REQUIRED != 0)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handover::record::{REQUIRED, Writer};
    use alloc::vec::Vec;

    fn desc_of(kinds: &[Kind], framing: (u32, u32), layout: u32) -> Vec<u8> {
        let d: [u8; DESC_HEADER + 8 * 40] = desc(framing, layout, 0b11, "test", kinds);
        d[..DESC_HEADER + 8 * count_known(kinds)].to_vec()
    }

    #[test]
    fn the_note_says_what_the_table_says() {
        // The note's header, then the description, as this kernel reads it.
        assert_eq!(u32::from_le_bytes(note().0[0..4].try_into().unwrap()), 5);
        assert_eq!(u32::from_le_bytes(note().0[8..12].try_into().unwrap()), NOTE_TYPE);
        assert_eq!(&note().0[12..17], b"GALA\0");
        let mine = Reads::mine();
        assert_eq!(mine.build(), BUILD);
        assert_eq!(mine.framing, (record::MIN_VERSION, record::VERSION));
        assert_eq!(mine.image_layout, IMAGE_LAYOUT);
        assert_eq!(mine.xstate, XSTATE);
        assert_eq!(mine.kinds(), COUNT);
        for k in KINDS.iter().filter(|k| k.writes != 0) {
            assert_eq!(mine.section(k.tag), Some((k.reads_from, k.writes)), "{}", k.name);
            assert!(k.must_read <= k.writes, "{}", k.name);
            assert_eq!(k.required, k.programs, "{}", k.name);
        }
        assert_eq!(mine.section(999), None);
        // A kernel hands over to its own build.
        assert_eq!(agree(&mine, true), Ok(()));
    }

    #[test]
    fn section_versions_match_their_writers() {
        assert_eq!(writes(tag::CLOCK), record::Clock::VERSION);
        assert_eq!(writes(tag::PLATFORM), record::Platform::VERSION);
        assert_eq!(writes(tag::FRAMEBUFFER), record::Framebuffer::VERSION);
        assert_eq!(writes(tag::CPUS), record::Cpus::VERSION);
        assert_eq!(writes(tag::VOLUME), record::Volume::VERSION);
        assert_eq!(known(tag::WRITER), NEXT);
        // Tags are unique.
        for (i, a) in KINDS.iter().enumerate() {
            assert!(KINDS[i + 1..].iter().all(|b| b.tag != a.tag), "{}", a.name);
        }
    }

    #[test]
    fn damaged_or_foreign_notes() {
        let mine = &note().0[20..];
        assert!(Reads::parse(mine).is_some());
        // Another note format.
        let mut other = mine.to_vec();
        other[0] = 2;
        assert!(Reads::parse(&other).is_none());
        // More kinds claimed than there are bytes.
        let mut short = mine.to_vec();
        short[24..28].copy_from_slice(&1000u32.to_le_bytes());
        assert!(Reads::parse(&short).is_none());
        assert!(Reads::parse(&mine[..20]).is_none());
    }

    #[test]
    fn the_kernel_before_notes() {
        let b = BEFORE_NOTES;
        assert_eq!(b.kinds(), 32);
        assert_eq!(b.section(tag::THREADS), Some((0, 2)));
        assert_eq!(b.section(tag::HANDLES), Some((0, 1)));
        assert_eq!(b.section(tag::WRITER), None);
        // This build's record is one it reads (THREADS 3 only adds at the
        // end, must_read 2), unless a section needs a newer reader.
        let r = agree(&b, true);
        if NEXT {
            assert!(matches!(r, Err(Mismatch::TooNew { tag: tag::PROCESSES, .. })));
        } else {
            assert_eq!(r, Ok(()));
        }
        assert_eq!(agree(&b, false), Ok(()));
    }

    #[test]
    fn refusals() {
        let mut kinds = KINDS;
        // A target that does not know HANDLES (REQUIRED).
        for k in kinds.iter_mut().filter(|k| k.tag == tag::HANDLES) {
            k.writes = 0;
        }
        let d = desc_of(&kinds, (1, 1), IMAGE_LAYOUT);
        let t = Reads::parse(&d).unwrap();
        assert!(matches!(agree(&t, true), Err(Mismatch::Unknown { tag: tag::HANDLES, .. })));
        // Without programs, nothing it lacks is needed.
        assert_eq!(agree(&t, false), Ok(()));
        // Reads THREADS only up to 1 (this kernel's must_read is 2).
        let mut kinds = KINDS;
        for k in kinds.iter_mut().filter(|k| k.tag == tag::THREADS) {
            k.writes = 1;
        }
        let d = desc_of(&kinds, (1, 1), IMAGE_LAYOUT);
        let t = Reads::parse(&d).unwrap();
        let e = agree(&t, true).unwrap_err();
        assert!(matches!(e, Mismatch::TooNew { tag: tag::THREADS, must_read: 2, reads: 1, .. }));
        assert!(alloc::format!("{e}").contains("reads THREADS up to version 1"));
        // Reads THREADS only from 4 on (this kernel's 3 is too old).
        for k in kinds.iter_mut().filter(|k| k.tag == tag::THREADS) {
            k.writes = 5;
            k.reads_from = 4;
        }
        let d = desc_of(&kinds, (1, 1), IMAGE_LAYOUT);
        let t = Reads::parse(&d).unwrap();
        assert!(matches!(agree(&t, true), Err(Mismatch::TooOld { tag: tag::THREADS, .. })));
        // Another framing.
        let d = desc_of(&KINDS, (2, 3), IMAGE_LAYOUT);
        let t = Reads::parse(&d).unwrap();
        assert!(matches!(agree(&t, false), Err(Mismatch::Framing { reads: (2, 3), .. })));
        // Another image layout: only matters with programs.
        let d = desc_of(&KINDS, (1, 1), IMAGE_LAYOUT + 1);
        let t = Reads::parse(&d).unwrap();
        assert!(matches!(agree(&t, true), Err(Mismatch::ImageLayout { .. })));
        assert_eq!(agree(&t, false), Ok(()));
        // Extended state.
        assert_eq!(agree_xstate(&t, 0b11), Ok(()));
        assert!(matches!(agree_xstate(&t, 0b111), Err(Mismatch::ExtendedState { .. })));
    }

    /// R9 (e): a kernel that does not read SOCKETS (or NET) takes programs
    /// without sockets; a record that carries sockets is refused for it
    /// before the point of no return.
    #[test]
    fn sockets_need_a_kernel_that_reads_them() {
        let mut kinds = KINDS;
        for k in kinds.iter_mut().filter(|k| matches!(k.tag, tag::SOCKETS | tag::NET)) {
            k.writes = 0;
        }
        let d = desc_of(&kinds, (1, 1), IMAGE_LAYOUT);
        let old = Reads::parse(&d).unwrap();
        assert_eq!(agree(&old, true), Ok(()));
        // The record as written with sockets in it.
        let mut w = Writer::new(1).unwrap();
        w.section_needing(tag::THREADS, 3, REQUIRED, 2).unwrap();
        crate::handover::sockets::write_net(&mut w, &Default::default()).unwrap();
        crate::handover::sockets::sockets_section(&mut w, &[]).unwrap();
        let bytes = w.finish().unwrap();
        let e = check_record(&bytes, &old).unwrap_err();
        assert!(matches!(e, Mismatch::Unknown { tag: tag::NET, .. }), "{e:?}");
        assert!(alloc::format!("{e}").contains("does not know section NET"));
        assert_eq!(check_record(&bytes, &Reads::mine()), Ok(()));
        // The kernel before notes: the same.
        assert!(matches!(
            check_record(&bytes, &BEFORE_NOTES),
            Err(Mismatch::Unknown { tag: tag::NET, .. })
        ));
    }

    #[test]
    fn records_are_checked_as_written() {
        let target = Reads::mine();
        let mut w = Writer::new(1).unwrap();
        w.section(tag::CMDLINE, 1, 0).unwrap();
        w.section_needing(tag::THREADS, 3, REQUIRED, 2).unwrap();
        w.section(500, 1, 0).unwrap(); // optional, unknown: skipped there
        let bytes = w.finish().unwrap();
        assert_eq!(check_record(&bytes, &target), Ok(()));
        // A section that needs a newer reader than the target.
        let mut w = Writer::new(1).unwrap();
        w.section_needing(tag::THREADS, 7, REQUIRED, 7).unwrap();
        let bytes = w.finish().unwrap();
        assert!(matches!(
            check_record(&bytes, &target),
            Err(Mismatch::TooNew { tag: tag::THREADS, version: 7, must_read: 7, .. })
        ));
        // An optional one: skipped by the target, accepted.
        let mut w = Writer::new(1).unwrap();
        w.section_needing(tag::CLOCK, 9, 0, 9).unwrap();
        let bytes = w.finish().unwrap();
        assert_eq!(check_record(&bytes, &target), Ok(()));
    }
}
