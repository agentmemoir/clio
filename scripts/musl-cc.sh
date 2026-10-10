#!/bin/sh
# Copyright 2026 Agent Memoir (https://agentmemoir.com/)
# SPDX-License-Identifier: Apache-2.0
#
# musl C compiler wrapper for release builds.
#
# Why this exists:
#
#   sqlite-vec (a C dependency, built from vendored source) writes
#   `typedef u_int8_t uint8_t` and never includes <sys/types.h>. glibc declares
#   those BSD names in the headers sqlite-vec already pulls in, so the tree
#   builds for glibc. musl declares them only inside <sys/types.h>, and only
#   under _BSD_SOURCE or _GNU_SOURCE, so the same source fails to compile with
#   "unknown type name 'u_int8_t'".
#
#   The fix is to force-include that header. It is applied here rather than
#   through CFLAGS because `cc` passes CFLAGS to the assembler too, and a C
#   header preprocessed into a .S file does not assemble. This wrapper adds the
#   flag for C and C++ inputs and passes assembly through untouched, which is
#   what `ring`'s hand-written assembly needs.
#
# The flag is deliberately not applied to the glibc or macOS builds: they
# compile today without it, and a flag that changes a working build is a flag
# that can break it later.
#
#   MUSL_EXTRA_CFLAGS carries target-specific flags that must also reach only the
#   C compiler. The aarch64 musl build needs `-mno-outline-atomics` there: GCC
#   otherwise emits calls to __aarch64_ldadd4_sync and friends, which glibc
#   provides and musl does not, so linking the vendored libdbus fails with an
#   undefined reference. It is aarch64-only and would be rejected on x86_64, so
#   the release workflow sets it for that one target rather than here.
#
# Used by .github/workflows/release.yml for the two *-unknown-linux-musl
# targets. Not used by `make build`, `make test`, or CI, which are unaffected.

set -eu

# `musl-gcc` is the name Debian's musl-tools installs for the runner's own
# architecture, and both Linux release targets build on a runner native for
# their target, so the default is correct on each. MUSL_CC_REAL overrides it for
# a cross build, where the compiler is named after the target instead.
REAL_CC="${MUSL_CC_REAL:-musl-gcc}"
EXTRA_CFLAGS="${MUSL_EXTRA_CFLAGS:-}"

for arg in "$@"; do
    case "$arg" in
        # Hand-written assembly: no preprocessed C, so none of the C flags.
        *.S | *.s | *.asm)
            exec "$REAL_CC" "$@"
            ;;
    esac
done

# Unquoted on purpose: this is a flag list meant to split into words.
# shellcheck disable=SC2086
exec "$REAL_CC" -include sys/types.h $EXTRA_CFLAGS "$@"
