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
# not merely another program under $dest. The old test matched any executable
# beneath $dest, so a different program at $dest/<name> or $dest/<dir>/<name>
# read as the shim.
is_shim() {
    cand=$1
    want=$2
    [ -n "$cand" ] || return 1
    if [ "$cand" = "$dest/$want" ]; then
        return 0
    fi
    if [ -L "$cand" ] && [ "$(readlink "$cand" 2>/dev/null)" = "$shim" ]; then
        return 0
    fi
    return 1
}

# Resolve the current `name` in $shell with the given flags (for example -lc or
# -ic), printing only what the shell resolved. A profile that reads a terminal
# must not hang the install, so stdin is /dev/null and `timeout` is used when
# the host has it. Stdout alone is the answer: a profile that prints would only
# add lines, and `is_shim` accepts an exact path, so noise reads as inactive.
probe_shell() {
    flags=$1
    if command -v timeout >/dev/null 2>&1; then
        timeout 5 "$shell" "$flags" "command -v $name" </dev/null 2>/dev/null || true
    else
        "$shell" "$flags" "command -v $name" </dev/null 2>/dev/null || true
    fi
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
    # one of those reads as inactive here. Ask the login shell and the
    # interactive shell too, and judge the shim by what they resolve.
    shell=${SHELL:-/bin/sh}
    [ -x "$shell" ] || shell=/bin/sh
    inactive=0
    for name in cargo make; do
        here=$(command -v "$name" 2>/dev/null || true)
        if is_shim "$here" "$name"; then
            echo "gate-install: $name shim is active for this shell"
            continue
        fi
        via=""
        login=$(probe_shell -lc)
        if is_shim "$login" "$name"; then
            via=login
        else
            interactive=$(probe_shell -ic)
            if is_shim "$interactive" "$name"; then
                via=interactive
            fi
        fi
        if [ -n "$via" ]; then
            echo "gate-install: $name shim is active in a $via shell; this non-interactive shell skips the profile that adds it"
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
