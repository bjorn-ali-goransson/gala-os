# Test image for gala-os: everything `cargo xtask test` needs, so CI, agents
# and developer machines run the same environment.
#
#   cargo xtask docker-test        (builds this image and runs the tests)
#
# The repository is mounted at /work; build output goes to target/docker.
# KVM is used when the host passes /dev/kvm through (docker-test does).
FROM ubuntu:24.04

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        qemu-system-x86 ovmf xorriso git make gcc libc6-dev curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# The toolchain pinned in rust-toolchain.toml (keep in sync).
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --default-toolchain 1.97.0 \
        --component rustfmt --component clippy \
        --target x86_64-unknown-none --target x86_64-pc-windows-msvc \
    && chmod -R a+rwX /opt/rustup /opt/cargo

ENV CARGO_TARGET_DIR=/work/target/docker
WORKDIR /work
CMD ["cargo", "xtask", "test"]
