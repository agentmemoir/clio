# A Linux test box for hardened `noexec` checks, run from a macOS host.
#
# Built once by `docker compose -f compose.dev.yaml build`; the image is then
# reused. On Linux, run the checks on the host instead.
FROM ubuntu:24.04

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      bash \
      ca-certificates \
      curl \
      git \
      make \
      python3 \
      python3-yaml \
      tmux \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
