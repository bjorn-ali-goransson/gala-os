# Notes for AI agents

- Read `docs/PROJECT_BRIEF.md` and `docs/CLEAN_ROOM.md` first. The clean-room
  rules are mandatory: never use disassembly, decompiled code, or leaked
  Windows source. Then `docs/WORKFLOW.md` (how a chunk of work is done,
  pitfalls, where things are), `docs/ROADMAP.md` (status and agreed order)
  and the sections of `docs/ARCHITECTURE.md` you touch (and `docs/SECURITY.md`,
  the security model's design, for tokens, descriptors and access checks).
- Observability carries great weight (the owner): every change ships with
  what would explain its failures (logs, events, what a failure leaves
  behind); see the rule at the top of `docs/OBSERVABILITY.md`.
- Structural, root-cause quality (the owner): fix the cause,
  not the symptom. No widened timeouts, retries, exclusions or workarounds
  in place of understanding; when a stopgap is unavoidable it is named as
  one, tracked in ROADMAP, and replaced. Examples that set the bar: the
  JVM corruption traced to QEMU's TCG and fixed by pinning a QEMU without
  the bug (not a kernel workaround); handover tests bounded by the
  measured pause (not a wider slack); smpsched_test counting context
  switches (not wall time).
- Keep the docs current in the same commit as the code: ROADMAP (status
  and next steps), ARCHITECTURE (design, invariants, limits), WORKFLOW
  (commands, conventions, pitfalls). The next session only knows what is
  written down.
- The docs describe the present, each for its purpose (README's
  "Documentation" table): no devlog, no dated progress notes, no
  "(the owner, <date>)" tags, no investigation stories. When something is
  done, update its ROADMAP item and the design section in place instead
  of appending a paragraph; a resolved bug leaves only its lesson (an
  invariant or a pitfall). Git history keeps the story.
- **Test first.** Every piece of work starts from a test program in
  `user/<feature>_test/` (a freestanding Windows .exe built on `user/ntrt`)
  whose expected results are those of real Windows. Write it first, add its
  boot scenario in `xtask/src/main.rs`, see it fail, then implement until it
  passes. Kernel-internal pieces also get host unit tests.
- Test program sources are checked in, and so are their binaries in
  `user/bin/` (run `cargo xtask export-tests` and commit the result; CI
  checks they match the sources).
- Record any borrowed code or new dependency in `THIRD_PARTY.md` in the same
  commit.
- `unsafe` is only allowed in `kernel/src/arch`, `kernel/src/boot`,
  `kernel/src/mm/heap.rs` and `kernel/src/sched/switch.rs` (and later the NT ABI/syscall layer), and in gala-um's platform
  (`kernel/src/hosted`, the `hosted` feature). Test programs in `user/` are
  freestanding Windows binaries and may use `unsafe` freely. Every
  `unsafe` block needs a `SAFETY:` comment.
- Allocation must be fallible; the kernel must never panic on OOM. Use
  `mm::heap::try_box`, `TryVec::try_push` etc., `try_reserve`. Infallible
  APIs are banned by `kernel/clippy.toml`.
- Pinned Windows build: Windows 11 24H2, build 26100 (tentative). Syscall
  numbers live in `kernel/src/nt/syscalls.rs` and `user/ntrt/src/lib.rs`
  and must match.
- Locks that may be held while preemptible must be `sync::IrqMutex`.
- Before committing: `cargo fmt --all`, the clippy commands in
  `.github/workflows/ci.yml`, and `cargo xtask test`.
