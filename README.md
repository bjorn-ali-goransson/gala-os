# gala-os

A from-scratch x86_64 kernel in Rust that aims to be binary-compatible with
Windows at the user-mode ABI level. Private project.

- Goals, milestones and rules: [`docs/PROJECT_BRIEF.md`](docs/PROJECT_BRIEF.md)
- Clean-room process (mandatory): [`docs/CLEAN_ROOM.md`](docs/CLEAN_ROOM.md)
- How the code is organized: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- Third-party code and provenance: [`THIRD_PARTY.md`](THIRD_PARTY.md)

**Status:** milestone 1 — boots via Limine on UEFI in QEMU, logs to serial,
reads and validates the memory map.

## Requirements

- Rust (the pinned toolchain in `rust-toolchain.toml` is installed by rustup
  automatically, including the `x86_64-unknown-none` target)
- `git`, `xorriso`, `qemu-system-x86_64`, OVMF UEFI firmware

Debian/Ubuntu: `sudo apt install qemu-system-x86 ovmf xorriso`

If OVMF is somewhere unusual, set `OVMF_CODE` (and optionally `OVMF_VARS`).

## Usage

```sh
cargo xtask build          # kernel + target/debug/gala-os.iso
cargo xtask run            # boot in QEMU, serial on stdio (add --headless for no window)
cargo xtask test           # host unit tests + headless QEMU boot test
cargo xtask run --release  # any command takes --release
```

The first build clones the pinned Limine release into `target/`. QEMU uses
KVM if `/dev/kvm` is available and falls back to TCG otherwise.
