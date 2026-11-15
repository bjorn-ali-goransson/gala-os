# Test image for gala-os: everything `cargo xtask test` needs, so CI, agents
# and developer machines run the same environment.
#
#   cargo xtask docker-test        (builds this image and runs the tests)
#
# The repository is mounted at /work; build output goes to target/docker.
# KVM is used when the host passes /dev/kvm through (docker-test does).
FROM ubuntu:26.04

# Behind a TLS-intercepting proxy, docker-test puts its CA here
# (XTASK_DOCKER_CA, or SSL_CERT_FILE); proxy variables come as build args.
COPY extra-ca/ /usr/local/share/ca-certificates/extra/
ARG http_proxy https_proxy no_proxy HTTP_PROXY HTTPS_PROXY NO_PROXY

# No QEMU package: the distribution's is 10.2.1, which wedges under KVM
# before the kernel logs a phase (WORKFLOW "QEMU"), and a container that
# carries one uses it silently when the pinned build is missing. These are what
# `cargo xtask qemu` needs to build QEMU_VERSION instead, into the
# ~/.cache/gala-qemu that docker-test mounts.

# The second group is what the tests run on the host side, next to the
# VM: the ssh and sftp clients and psql for the server scenarios, 7z and
# unzip for the prebuilt programs that come as archives (Mesa, FileZilla,
# the Vulkan runtime, git's curl), msitools for the installer ones,
# musl-gcc, weston and Xvfb for the Linux-binary ones, shared-mime-info
# and libglib2.0-bin (glib-compile-schemas) for the desktop ones. They are what CI's runner installs
# (`.github/workflows/ci.yml`); without one the scenario that needs it
# fails -- after a full boot where it is a client.
#
# The base image takes the plain amd64 archive (it sets no
# APT::Architecture-Variants), so its gcc is the one user/lx/bin is built
# with: an amd64v3 toolchain defaults to -march=x86-64-v3 and gives other
# bytes, which user/lx/build.sh refuses (WORKFLOW "Linux test programs").
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ovmf xorriso git make gcc libc6-dev curl ca-certificates \
        python3 python3-venv ninja-build pkg-config flex bison bzip2 xz-utils \
        libglib2.0-dev libpixman-1-dev libslirp-dev libfdt-dev zlib1g-dev \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y \
        7zip unzip openssh-client postgresql-client msitools musl-tools \
        weston xvfb libgtk-3-0t64 shared-mime-info libglib2.0-bin \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# The repository is bind-mounted from the host, owned by the host's user,
# while the container is root: git then refuses to read it ("detected
# dubious ownership") and every revision it is asked for comes back empty
# (the Limine checkout's, for one). The container has nothing of its own
# in /work, so the tree it is given is a tree it may read.
RUN git config --system --add safe.directory '*'

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
