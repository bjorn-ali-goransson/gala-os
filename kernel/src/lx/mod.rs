//! The Linux personality's groundwork (ROADMAP LX milestone 1; design:
//! ARCHITECTURE "Plan: a second personality (Linux)"; sources:
//! CLEAN_ROOM.md "Linux").
//!
//! Plain data processing only, nothing wired in yet: no system-call entry
//! reaches this module and nothing here touches memory, processes or
//! devices. Each piece is checked by host unit tests against what real
//! Linux does (user/lx's programs, whose transcripts `cargo xtask
//! lx-expect` records on Linux):
//! - [`elf`]: ELF64 executables parsed and validated, and their load plan.
//! - [`stack`]: the initial process stack (argc, argv, envp, auxv,
//!   strings) as bytes.
//! - [`syscalls`] and [`errno`]: Linux x86_64 system-call numbers and
//!   errno values, name <-> number.
//!
//! The exec path and the dispatcher plug in later (the per-process
//! personality seam, ROADMAP LX step 2): exec parses the image
//! (`ElfImage::parse`), chooses a base (`load_bias`), maps the plan's
//! segments, builds the stack (`stack::build`) at the stack's top and
//! starts the thread at the plan's entry (or the interpreter's) with rsp
//! at `InitialStack::sp`; the dispatcher's table is indexed by
//! [`syscalls::nr`] and names unknown calls with [`syscalls::name`].

pub mod elf;
pub mod errno;
pub mod stack;
pub mod syscalls;
