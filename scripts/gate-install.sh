#!/bin/sh
# Install or remove the gate shim symlinks (`cargo`, `make`) so heavy
# commands in a Clio checkout go through scripts/gate.py. The shims pass
# through unchanged outside a Clio checkout and with GATE_RAW=1.
#
# Usage:
#   scripts/gate-install.sh [--dest DIR]     (default: ~/.local/bin)
#   scripts/gate-install.sh --uninstall [--dest DIR]
set -eu

dest=${GATE_SHIM_DIR:-$HOME/.local/bin}
mode=install
while [ $# -gt 0 ]; do
    case "$1" in
        --dest)
            [ $# -ge 2 ] || { echo "gate-install: --dest needs a directory" >&2; exit 2; }
            dest=$2
            shift 2
            ;;
        --uninstall)
            mode=uninstall
            shift
            ;;
        -h | --help)
            printf '%s\n' \
                'usage: gate-install.sh [--dest DIR]' \
                '       gate-install.sh --uninstall [--dest DIR]'
            exit 0
            ;;
        *)
            echo "gate-install: unknown option: $1" >&2
            exit 2
            ;;
    esac
done

repo_root=$(cd "$(dirname "$0")/.." && pwd -P)
shim="$repo_root/scripts/gate-shim"
[ -f "$shim" ] || {
    echo "gate-install: missing $shim" >&2
    exit 1
}
mkdir -p "$dest"

for name in cargo make; do
    link="$dest/$name"
    if [ "$mode" = uninstall ]; then
        if [ -L "$link" ] && [ "$(readlink "$link")" = "$shim" ]; then
            rm "$link"
            echo "removed $link"
        fi
        continue
    fi
    if [ -e "$link" ] && [ ! -L "$link" ]; then
        echo "gate-install: $link exists and is not a symlink; leaving it alone" >&2
        exit 1
    fi
    chmod +x "$shim"
    ln -sfn "$shim" "$link"
    echo "linked $link -> $shim"
done

# Whether a path resolved by `command -v` is the shim this install linked, and
# not merely any program under $dest. The old test matched any executable
# beneath $dest, so a different program at $dest/<name> or $dest/<dir>/<name>
# read as the shim. Only a link whose target is the shim counts.
is_shim() {
    cand=$1
    [ -n "$cand" ] || return 1
    if [ -L "$cand" ] && [ "$(readlink "$cand" 2>/dev/null)" = "$shim" ]; then
        return 0
    fi
    return 1
}

# Resolve the current `name` in $shell with the given flags (for example -lc or
# -ic), printing the first line that is a symlink to the shim. A profile may
# print to stdout, so each line is tested rather than the whole output; a
# profile that reads a terminal must not hang the install, so stdin is /dev/null
# and `timeout` is used when the host has it. A killed probe returns 124, which
# the caller reads as "could not probe" rather than "the shim is inactive".
probe_shell() {
    flags=$1
    rc=0
    if command -v timeout >/dev/null 2>&1; then
        out=$(timeout 5 "$shell" "$flags" "command -v $name" </dev/null 2>/dev/null) || rc=$?
    else
        out=$("$shell" "$flags" "command -v $name" </dev/null 2>/dev/null) || rc=$?
    fi
    if [ "$rc" = 124 ]; then
        return 124
    fi
    printf '%s\n' "$out" | while IFS= read -r line; do
        if is_shim "$line"; then
            printf '%s\n' "$line"
            break
        fi
    done
}

# Run a lookup under the same 5s budget as the profile probes when the host
# has `timeout`, so a slow NSS backend cannot hang the install.
lookup() {
    if command -v timeout >/dev/null 2>&1; then
        timeout 5 "$@"
    else
        "$@"
    fi
}

# The account's login shell: the shell the machine runs for the user, and the
# shell whose profile carries the PATH line. getent is the Linux query, dscl
# the macOS one. Prints nothing and fails when neither can answer.
login_shell() {
    account=$(id -un 2>/dev/null) || return 1
    [ -n "$account" ] || return 1
    found=
    if command -v getent >/dev/null 2>&1; then
        found=$(lookup getent passwd "$account" 2>/dev/null | cut -d: -f7 | head -n 1)
    fi
    if [ -z "$found" ] && command -v dscl >/dev/null 2>&1; then
        found=$(lookup dscl . -read "/Users/$account" UserShell 2>/dev/null | awk 'NR == 1 { sub(/^UserShell:[[:space:]]*/, ""); print }')
    fi
    [ -n "$found" ] || return 1
    printf '%s\n' "$found"
}

if [ "$mode" = install ]; then
    # A symlink in a directory that comes *after* the real tool on PATH never
    # runs, and the old containment test ("PATH contains $dest") reported
    # success anyway. Resolve each name and say where it landed, so a shim that
    # is installed but shadowed reads as inactive rather than as a clean install.
    # A dead shim is a failed install, so this exits non-zero: a caller that
    # checks the exit code must not read the dead state as success.
    #
    # This shell cannot be the whole test. Over plain `ssh` it reads neither the
    # login profile nor the interactive profile, so a machine that adds $dest in
    # one of those reads as inactive here. Ask both profile shells. The
    # interactive shell is asked even when the login shell hits, because a login
    # hit with an interactive shadow would otherwise print a bare "active" and
    # hide the shell a user actually runs gates in.
    #
    # A profile slower than the 5s probe budget is killed by `timeout` (status
    # 124). That is "could not probe", not "the shim is inactive": a slow
    # profile must not fail an otherwise correct install, the same false FAIL
    # #213 removes.
    #
    # The bare /bin/sh fallback is dash on most Linux hosts, and dash reads
    # neither ~/.bashrc nor ~/.zshrc, so it failed a machine set up the way this
    # install sets one up. Resolve the account's login shell instead: that is the
    # shell the machine runs for the user and the shell whose profile carries
    # the PATH line. Keep /bin/sh only as the last resort.
    shell=${SHELL:-}
    if [ -z "$shell" ] || [ ! -x "$shell" ]; then
        shell=$(login_shell) || shell=
    fi
    [ -n "$shell" ] && [ -x "$shell" ] || shell=/bin/sh
    inactive=0
    for name in cargo make; do
        here=$(command -v "$name" 2>/dev/null || true)
        if is_shim "$here"; then
            echo "gate-install: $name shim is active for this shell"
            continue
        fi
        login_rc=0
        login=$(probe_shell -lc) || login_rc=$?
        interactive_rc=0
        interactive=$(probe_shell -ic) || interactive_rc=$?
        if [ -n "$login" ] || [ -n "$interactive" ]; then
            where="the login and interactive shells"
            [ -n "$login" ] || where="the interactive shell"
            [ -n "$interactive" ] || where="the login shell"
            echo "gate-install: $name shim is active in $where; this non-interactive shell skips the profile that adds it"
            if [ -n "$login" ] && [ -z "$interactive" ]; then
                if [ "$interactive_rc" = 124 ]; then
                    echo "gate-install: note: could not probe the interactive shell for $name within the 5s budget"
                else
                    echo "gate-install: note: $name is not active in the interactive shell; a tool earlier on your interactive PATH shadows the shim"
                fi
            fi
            if [ -z "$login" ] && [ -n "$interactive" ]; then
                if [ "$login_rc" = 124 ]; then
                    echo "gate-install: note: could not probe the login shell for $name within the 5s budget"
                else
                    echo "gate-install: note: $name is not active in the login shell; only the interactive profile adds $dest"
                fi
            fi
            continue
        fi
        if [ "$login_rc" = 124 ] || [ "$interactive_rc" = 124 ]; then
            echo "gate-install: $name: a shell profile did not finish within the 5s probe budget; a slow profile is not read as inactive, so the install is not failed"
            continue
        fi
        if [ -z "$here" ]; then
            printf 'gate-install: NOT ACTIVE: %s is not on PATH; add %s to your shell profile:\n' "$name" "$dest"
        else
            printf 'gate-install: NOT ACTIVE: this shell resolves %s as %s; %s comes later on PATH\n' "$name" "$here" "$dest"
        fi
        printf '  export PATH="%s:$PATH"\n' "$dest"
        inactive=1
    done
    if [ "$inactive" = 1 ]; then
        echo "gate-install: the shim is linked but not active; fix PATH and run this again" >&2
        exit 1
    fi
fi
