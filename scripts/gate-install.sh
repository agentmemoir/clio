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

if [ "$mode" = install ]; then
    # A symlink in a directory that comes *after* the real tool on PATH never
    # runs, and the old containment test ("PATH contains $dest") reported
    # success anyway. Resolve each name and say where it landed, so a shim that
    # is installed but shadowed reads as inactive rather than as a clean install.
    for name in cargo make; do
        resolved=$(command -v "$name" 2>/dev/null || true)
        case "$resolved" in
            "$dest"/*) echo "gate-install: $name shim is active for this shell" ;;
            "")
                printf 'gate-install: NOT ACTIVE: %s is not on PATH; add %s to your shell profile:\n' "$name" "$dest"
                printf '  export PATH="%s:$PATH"\n' "$dest"
                ;;
            *)
                printf 'gate-install: NOT ACTIVE: this shell resolves %s as %s; %s comes later on PATH\n' "$name" "$resolved" "$dest"
                printf '  export PATH="%s:$PATH"\n' "$dest"
                ;;
        esac
    done
fi
