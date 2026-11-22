#!/bin/sh
# Copyright 2026 Agent Memoir (https://agentmemoir.com/)
# SPDX-License-Identifier: Apache-2.0
#
# clio installer.
#
# Installs the prebuilt clio binary for this machine into a directory the user
# owns. No admin rights, no compiler, no package manager.
#
#   curl -fsSL https://github.com/agentmemoir/clio/releases/latest/download/install.sh | sh
#
# Override the target directory with CLIO_INSTALL_DIR, and the release tag with
# CLIO_VERSION. Both are read from the environment, so a pinned install is:
#
#   CLIO_VERSION=v0.1.0 sh install.sh
#
# POSIX sh only: this runs under dash, busybox ash, and bash alike.

set -eu

REPO="${CLIO_REPO:-agentmemoir/clio}"
VERSION="${CLIO_VERSION:-latest}"
BIN_NAME="clio"

# The release job signs checksums.txt with the matching private key, and this
# key is the trust root for every install: the manifest and the asset come from
# one download location, so the manifest's hashes alone prove only that both
# files arrived intact, not that the release job wrote them. The key is public
# and ships here on purpose.
CLIO_RELEASE_PUBKEY='-----BEGIN PUBLIC KEY-----
MIIBojANBgkqhkiG9w0BAQEFAAOCAY8AMIIBigKCAYEA3sDFW6I7yzZD9akixmkB
Iy45Jjx+uRujk/6pAW5yylZ1L5touxNl86OEOV0HGwzoVTQxgyC3LGuNmTl1+rVg
XxxpDEBHt2QocL4tIFVEOcF2vHtr6voQQU/o1lPTx4C3FwM5Z+coQtvGPxNwjsM4
BXPNbmM7CmHYIrwhDoJXlgnG4nFf+dy0cl9baYZ4yb/nce5SOpQotZY7vStYezm/
5hSD7t/MS40YSLpRAnNQ2Msb8onOfuyu+NXDRVVCEN45zVJlJrGnyKt9lLfAWf+x
JlredGKqkmQYd2Iu7EczBFl14MDYUVd7GpCdYWokAnNt8mrZfTJEkVjVv9HmH4fi
wMKWhZW2o7rpO7XwASbnZgn/6VYzdDsa5Of9Xdg4LCQaRQb57st7cHV/KMO1HNhv
ZMFbLPYEQPe2d5Q0oM2B4Tp35HNqB5aYvdpkor9RJ2Ety1Ix53vdxttn2HfhIlD3
kLJC3lmz0AgQ+w+852Jav1RoFDAsTgLjLWfQABhTIpz7AgMBAAE=
-----END PUBLIC KEY-----'

# Written to stderr, so a `curl` install piped into `sh` leaves nothing on stdout for a
# caller who is redirecting it.
log() { printf '%s\n' "$*" >&2; }
die() {
    log "error: $*"
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || die "$1 is required but was not found on this machine"
}

# --- machine detection -------------------------------------------------------

# `uname -m` does not agree on names. Linux says aarch64 and x86_64, macOS says
# arm64 and x86_64, and older 32-bit ARM Linux kernels say armv7l. Every branch
# that can reach a shipped target is listed; anything else fails here with a
# readable message rather than 404 on a download.
detect_target() {
    os="$(uname -s)"
    machine="$(uname -m)"

    case "$os" in
        Darwin)
            case "$machine" in
                arm64) printf 'aarch64-apple-darwin' ;;
                x86_64) die "macOS on x86_64 is no longer supported. Apple Silicon (arm64) is required. If running on Apple Silicon under Rosetta, re-run with: arch -arm64 sh install.sh" ;;
                *) die "unsupported macOS architecture: $machine" ;;
            esac
            ;;
        Linux)
            case "$machine" in
                x86_64 | amd64) printf 'x86_64-unknown-linux-musl' ;;
                aarch64 | arm64) printf 'aarch64-unknown-linux-musl' ;;
                armv7l | armv7) die "unsupported 32-bit ARM Linux; clio ships 64-bit builds only" ;;
                *) die "unsupported Linux architecture: $machine" ;;
            esac
            ;;
        *)
            die "unsupported operating system: $os. clio ships macOS and Linux builds."
            ;;
    esac
}

# --- download ----------------------------------------------------------------

# curl and wget spell the flags differently, so this is the one place that
# knows about both. `--proto` and `--tlsv1.2` are curl-only guards that pin the
# transfer to HTTPS 1.2 or better; a redirect to another scheme is refused
# rather than followed.
download() {
    url="$1"
    out="$2"
    if command -v curl >/dev/null 2>&1; then
        curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$out"
    elif command -v wget >/dev/null 2>&1; then
        wget -q --https-only "$url" -O "$out"
    else
        die "neither curl nor wget was found. Install one of them and re-run."
    fi
}

# --- checksum ----------------------------------------------------------------

# One of these is on every machine that can run the binary: coreutils on Linux,
# shasum from perl on macOS, and openssl as a fallback for a minimal container.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$1" | sed 's/^.*= *//'
    else
        die "no sha256sum, shasum, or openssl found, so the download cannot be verified"
    fi
}

# --- signature ---------------------------------------------------------------

# The manifest is fetched from the same place as the asset, so its hashes cannot
# vouch for themselves. This verifies the detached RSA signature that the
# release job makes over checksums.txt, using the key built into this file.
# A failed or impossible verification stops the install: a release that cannot
# be checked is not installed.
verify_manifest() {
    manifest="$1"
    signature="$2"

    command -v openssl >/dev/null 2>&1 \
        || die "openssl is required to check the release signature, and it was not found. Install openssl and re-run."
    [ -s "$signature" ] \
        || die "the release signature for checksums.txt is missing or empty. The release is incomplete, so nothing was installed."

    keyfile="$workdir/release-signing-key.pub.pem"
    printf '%s\n' "$CLIO_RELEASE_PUBKEY" > "$keyfile"
    openssl dgst -sha256 -verify "$keyfile" -signature "$signature" "$manifest" >/dev/null 2>&1 \
        || die "the signature on checksums.txt does not match the key built into this installer. The manifest was changed, or it came from somewhere other than this release. Nothing was installed."
}

# --- main --------------------------------------------------------------------

main() {
    need uname
    need tar
    need mktemp

    # Resolved here rather than at the top of the file so an unset HOME gets the
    # message below instead of `set -u` aborting on an unbound variable, which is
    # the kind of failure a user cannot act on.
    if [ -n "${CLIO_INSTALL_DIR:-}" ]; then
        INSTALL_DIR="$CLIO_INSTALL_DIR"
    elif [ -n "${HOME:-}" ]; then
        INSTALL_DIR="$HOME/.local/bin"
    else
        die "HOME is not set, so there is no user directory to install into. Set CLIO_INSTALL_DIR to a directory you own and re-run."
    fi

    target="$(detect_target)"
    asset="$BIN_NAME-$target"
    base="https://github.com/$REPO/releases"

    if [ "$VERSION" = "latest" ]; then
        release_url="$base/latest/download"
    else
        case "$VERSION" in
            v*) tag="$VERSION" ;;
            *) tag="v$VERSION" ;;
        esac
        release_url="$base/download/$tag"
    fi

    workdir="$(mktemp -d)"
    # shellcheck disable=SC2064 # expand $workdir now, not at trap time
    trap "rm -rf '$workdir'" EXIT INT TERM

    log "clio $VERSION ($target)"

    log "downloading checksums"
    download "$release_url/checksums.txt" "$workdir/checksums.txt" \
        || die "could not fetch the checksums for $VERSION. Is there a release for this version?"

    log "downloading checksums signature"
    download "$release_url/checksums.txt.sig" "$workdir/checksums.txt.sig" \
        || die "could not fetch the signature for checksums.txt for $VERSION"

    verify_manifest "$workdir/checksums.txt" "$workdir/checksums.txt.sig"
    log "signature ok"

    log "downloading $asset"
    download "$release_url/$asset.tar.gz" "$workdir/$asset.tar.gz" \
        || die "could not download $asset.tar.gz for $VERSION"

    # The expected line is read from the file rather than recomputed, so a
    # missing target is a mismatch and not a silent skip.
    expected="$(awk -v f="$asset.tar.gz" '$2 == f || $2 == "*"f { print $1 }' "$workdir/checksums.txt")"
    [ -n "$expected" ] || die "$asset.tar.gz is not listed in checksums.txt; refusing to install an unverified download"

    actual="$(sha256_of "$workdir/$asset.tar.gz")"
    if [ "$expected" != "$actual" ]; then
        die "checksum mismatch for $asset.tar.gz
  expected $expected
  actual   $actual
The download was not installed. Re-run, and if it fails again the file on the
release is wrong or the transfer was altered."
    fi
    log "checksum ok"

    tar -C "$workdir" -xzf "$workdir/$asset.tar.gz"
    [ -x "$workdir/$BIN_NAME-$target/$BIN_NAME" ] || die "the archive did not contain the expected binary"

    mkdir -p "$INSTALL_DIR"
    # Install to a temporary name in the destination directory and rename, so a
    # download that is interrupted here cannot leave a half-written binary on
    # the user's PATH under the real name.
    cp "$workdir/$BIN_NAME-$target/$BIN_NAME" "$INSTALL_DIR/.$BIN_NAME.new"
    chmod 755 "$INSTALL_DIR/.$BIN_NAME.new"
    mv "$INSTALL_DIR/.$BIN_NAME.new" "$INSTALL_DIR/$BIN_NAME"

    log "installed $INSTALL_DIR/$BIN_NAME"

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            log ""
            log "$INSTALL_DIR is not on your PATH. Add this to your shell profile:"
            log "  export PATH=\"$INSTALL_DIR:\$PATH\""
            log "Then open a new terminal, or run that line now."
            ;;
    esac

    log ""
    log "Next: $BIN_NAME setup"
}

# The last line is the entry point. A test sources this file with
# CLIO_INSTALL_LIB_ONLY=1 to call one function on its own, so the guard is the
# only thing that may be conditional.
[ "${CLIO_INSTALL_LIB_ONLY:-0}" = "1" ] || main "$@"
