#!/bin/sh
# Run the real gate installer on this host, the way a Mac user runs it, and
# accept only the outcomes that can happen.
#
# The installer links the shim into a destination and then asks a login shell
# and an interactive shell whether the `cargo` and `make` they resolve are the
# shim. That question needs a wall-clock watchdog, because a profile that
# never finishes must not hold the install. GNU coreutils provides the
# watchdog as `timeout` on Linux and as `gtimeout` on macOS, so a host has
# one, both, or neither.
#
# There are two real outcomes and this script accepts only those two. A host
# with a watchdog lets the installer confirm the shim, so the installer must
# exit zero and say the shim is active. A host without a watchdog cannot
# probe a profile, so the installer must exit non-zero and say that profile
# verification is unavailable. Any other outcome fails this script.
#
# The home is a temporary one whose profile adds the install destination to
# PATH, so the installer reads a profile that carries the PATH line, the same
# way the self-test does. The host PATH is left alone, so the answer is the
# answer a user on this host would get.
set -eu

repo_root=$(cd "$(dirname "$0")/.." && pwd -P)
installer="$repo_root/scripts/gate-install.sh"
if [ ! -f "$installer" ]; then
    echo "gate-install-runner-check: missing $installer" >&2
    exit 1
fi

# The shell the installer probes. The installer uses $SHELL when it is a shell
# it can run, so this script sets it to one it has checked. A login bash reads
# ~/.profile, and /bin/sh on macOS is bash in POSIX mode and reads it too.
shell=/bin/sh
if [ -x /bin/bash ]; then
    shell=/bin/bash
fi

# The watchdog, when the host has one. The installer looks for the same two
# names in the same order, so both answers are about the same host.
tool=none
if command -v timeout >/dev/null 2>&1; then
    tool=timeout
elif command -v gtimeout >/dev/null 2>&1; then
    tool=gtimeout
fi

tmp_root=${TMPDIR:-/tmp}
work=$(mktemp -d "${tmp_root%/}/gate-install-runner.XXXXXX")

# The temporary directories go away on every exit, including a failure.
cleanup() {
    rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

dest="$work/dest"
home="$work/home"
mkdir -p "$dest" "$home"
printf 'export PATH="%s:$PATH"\n' "$dest" > "$home/.profile"

echo "the timeout tool on this host: $tool"
echo "the shell the installer probes: $shell"

# The run is bounded in both branches, and neither bound needs a background
# job. With a watchdog this script wraps the installer in it and gives it 120
# seconds, and the installer bounds each profile probe with the same tool.
# Without a watchdog the installer runs no profile code at all: it reports
# that it cannot probe a profile and stops, so no profile can hold the run,
# and the only work left is linking two symlinks. The timeout of the job that
# runs this script is the outer bound for that branch.
rc=0
if [ "$tool" = none ]; then
    out=$(HOME="$home" SHELL="$shell" sh "$installer" --dest "$dest" 2>&1) || rc=$?
else
    out=$(HOME="$home" SHELL="$shell" "$tool" 120 sh "$installer" --dest "$dest" 2>&1) || rc=$?
fi
printf '%s\n' "$out"
echo "the installer exit code: $rc"

if [ "$tool" = none ]; then
    echo "branch: the host has no timeout tool, so the installer cannot probe a profile"
    if [ "$rc" -eq 0 ]; then
        echo "the installer must fail when the host has no timeout tool" >&2
        exit 1
    fi
    case "$out" in
        *"profile verification is unavailable"*) ;;
        *)
            echo "the installer must report that profile verification is unavailable" >&2
            exit 1
            ;;
    esac
else
    echo "branch: the host has the timeout tool $tool, so the installer must confirm the shim"
    if [ "$rc" -ne 0 ]; then
        echo "the installer must succeed when the host has the timeout tool $tool" >&2
        exit 1
    fi
    case "$out" in
        *"shim is active"*) ;;
        *)
            echo "the installer must report that the shim is active" >&2
            exit 1
            ;;
    esac
fi

echo "the installer behaved as this host requires"
