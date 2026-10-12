# A Linux test box for hardened `noexec` checks, run from a macOS host.
#
# Built once by `docker compose -f compose.dev.yaml build`; the image is then
# reused. On Linux, run the checks on the host instead.
#
# The box carries a Rust toolchain, because the checks it exists for compile a
# program and then RUN it, and running it is the half a `noexec` mount breaks. A
# box with no compiler could only ever exercise the helper, never the check that
# dies. The toolchain is the one `rust-toolchain.toml` pins, read out of that
# file at build time so the image and the repository cannot disagree about which
# compiler the checks run under. There is no nextest here: this box runs the
# small `noexec` checks, not a crate test suite.
FROM ubuntu:24.04

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      bash \
      build-essential \
      ca-certificates \
      curl \
      git \
      make \
      python3 \
      python3-yaml \
      tmux \
 && rm -rf /var/lib/apt/lists/*

# `build-essential` above is not optional once a toolchain is present: rustc links
# every binary it produces through `cc`, so without a linker the instrumented
# compile these checks perform fails with "linker `cc` not found", which is a
# missing package and has nothing to do with the mount this box tests.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# The version is read from the pin rather than repeated here, so raising
# `rust-toolchain.toml` raises this box with it and the two cannot drift.
COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN pinned="$(sed -n 's/^channel = "\(.*\)"$/\1/p' /tmp/rust-toolchain.toml)" \
 && test -n "$pinned" \
 && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal \
          --default-toolchain "$pinned" \
 && rustup component add rustfmt clippy llvm-tools-preview \
 && rm -f /tmp/rust-toolchain.toml

WORKDIR /workspace
