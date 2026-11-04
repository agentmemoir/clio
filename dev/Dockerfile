# A Linux test box for hardened `noexec` checks, run from a macOS host.
#
# Built once by `docker compose -f compose.dev.yaml build`; the image is then
# reused. On Linux, run the checks on the host instead.
#
# The box carries a Rust toolchain, because the checks it exists for compile a
# program and then RUN it, and running it is the half a `noexec` mount breaks. A
# box with no compiler could only ever exercise the helper, never the check that
# dies. There is no nextest here: this box runs the small `noexec` checks, not a
# crate test suite.
FROM ubuntu:24.04@sha256:fe81560990fae86dc4743202b2cb4d3df98d0bcf949173cecef15ce1127ef852

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

# The channel and the component list both come from `rust-toolchain.toml` rather
# than being repeated here, so raising the pin raises this box with it and the two
# cannot drift. The `test -n` lines turn a parse that has stopped matching that
# file into a failed build, instead of an image that quietly holds the wrong
# compiler.
COPY rust-toolchain.toml /tmp/rust-toolchain.toml
# dev/Dockerfile installs a verified toolchain so the box is reproducible. The
# base image is pinned by digest; the tag is inert and the digest decides the
# content, so change both together or the digest silently keeps the old image.
#
# rustup-init is fetched from a version-pinned archive URL and checked against
# the SHA-256 that URL's own .sha256 file publishes. 1.29.3 was the release
# named by https://static.rust-lang.org/rustup/release-stable.toml when this pin
# was made. Bumping rustup moves three values together: the URL version and both
# per-architecture hashes. A missing entry or a mismatched hash fails the build.
#
# This verifies the bootstrap installer only. The compiler payload that rustup
# then downloads is fetched over TLS and is not hash-checked here.
ARG TARGETARCH
RUN channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' /tmp/rust-toolchain.toml)" \
 && components="$(sed -n 's/^components = \[\(.*\)\]$/\1/p' /tmp/rust-toolchain.toml \
      | tr -d '"' | tr ',' ' ')" \
 && test -n "$channel" \
 && test -n "$components" \
 && case "$TARGETARCH" in \
      amd64) rust_arch=x86_64-unknown-linux-gnu; rustup_sha256=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55 ;; \
      arm64) rust_arch=aarch64-unknown-linux-gnu; rustup_sha256=9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282 ;; \
      *) echo >&2 "unsupported TARGETARCH: $TARGETARCH"; exit 1 ;; \
    esac \
 && curl -fsSLo /tmp/rustup-init "https://static.rust-lang.org/rustup/archive/1.29.3/${rust_arch}/rustup-init" \
 && echo "${rustup_sha256}  /tmp/rustup-init" | sha256sum -c - \
 && chmod +x /tmp/rustup-init \
 && /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain "$channel" --default-host "$rust_arch" \
 && rm -f /tmp/rustup-init \
 && rustup component add $components \
 && rm -f /tmp/rust-toolchain.toml

WORKDIR /workspace
