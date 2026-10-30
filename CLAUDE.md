# Notes for AI agents

- Read `docs/PROJECT_BRIEF.md` and `docs/CLEAN_ROOM.md` first. The clean-room
  rules are mandatory: never use disassembly, decompiled code, or leaked
  Windows source.
- Record any borrowed code or new dependency in `THIRD_PARTY.md` in the same
  commit.
- `unsafe` is only allowed in `kernel/src/arch`, `kernel/src/boot`,
  `kernel/src/mm/heap.rs` and `kernel/src/sched/switch.rs` (and later the NT ABI/syscall layer). Every
  `unsafe` block needs a `SAFETY:` comment.
- Allocation must be fallible; the kernel must never panic on OOM. Use
  `mm::heap::try_box`, `TryVec::try_push` etc., `try_reserve`. Infallible
  APIs are banned by `kernel/clippy.toml`.
- Locks that may be held while preemptible must be `sync::IrqMutex`.
- Before committing: `cargo fmt --all`, the clippy commands in
  `.github/workflows/ci.yml`, and `cargo xtask test`.
