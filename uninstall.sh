#!/bin/sh
# Remove an installer-managed leanck by running `leanck _uninstall`, which reads
# the installer's inventory. Keep this script usable with curl | sh: everything
# runs inside main, so a truncated download removes nothing.

die() { printf 'leanck-uninstall: %s\n' "$*" >&2; exit 1; }
usage() {
    cat <<'EOF'
Usage: sh uninstall.sh [OPTIONS]

Remove installer-managed leanck files and host plugin registrations.
Modified files, user configuration, Lean projects, caches, and installer backups
are preserved. The recorded executable stops leanck daemons before removal.

  -y, --yes          Approve removal without prompting
  --dry-run          Preview only; change nothing and run no host commands
  --state-dir DIR    Installer state (default: $XDG_STATE_HOME/leanck/installer,
                     or ~/.local/state/leanck/installer)
  --plugin HOSTS     Also unregister claude, codex, or both (for older installs)
  --timeout SECONDS  Maximum time for each external command (default: 300)
  -h, --help         Show this help

Removal runs `leanck _uninstall` from the installed executable or, if that is
missing, from the plugin bundle, the release archive holding this script, or PATH.
EOF
}

main() {
    state=''
    if [ "$#" = 1 ] && [ "$1" = help ]; then usage; exit 0; fi
    previous=''
    for arg in "$@"; do
        case "$arg" in -h|--help) usage; exit 0 ;; --state-dir=*) state=${arg#*=} ;; esac
        [ "$previous" != --state-dir ] || state=$arg
        previous=$arg
    done
    [ -n "${HOME:-}" ] || die 'HOME must be set'
    state=${state:-${XDG_STATE_HOME:-$HOME/.local/state}/leanck/installer}
    case "$state" in /*) ;; "~"/*) state=$HOME/${state#"~"/} ;; *) state=$PWD/$state ;; esac
    tab=$(printf '\t')
    value() { LC_ALL=C awk -F "$tab" -v key="$1" '$1 == key { v = $2 } END { print v }' "$state/selections.tsv" 2>/dev/null; }
    data_dir=$(value data_dir) sibling=''
    case "$0" in uninstall.sh|*/uninstall.sh) sibling=$(dirname -- "$0")/plugin/bin/leanck ;; esac
    # The recorded executables first; a missing one is skipped, and `_uninstall`
    # then removes everything else the inventory records.
    candidates=$(
        value last_binary_path
        LC_ALL=C awk -F "$tab" '$1 == "file" && $3 ~ /\/leanck$/ { print $3 }' "$state/uninstall.tsv" 2>/dev/null
        printf '%s\n' "${data_dir:-${XDG_DATA_HOME:-$HOME/.local/share}/leanck}/bundle/plugin/bin/leanck"
        printf '%s\n' "$sibling"
        command -v leanck 2>/dev/null
    )
    set -f
    old_ifs=$IFS
    IFS='
'
    for leanck in $candidates; do
        IFS=$old_ifs
        if [ -f "$leanck" ] && [ -x "$leanck" ] && "$leanck" _uninstall --help >/dev/null 2>&1; then
            exec "$leanck" _uninstall "$@"
        fi
    done
    IFS=$old_ifs
    if [ ! -f "$state/uninstall.tsv" ] && [ ! -f "$state/selections.tsv" ]; then
        printf 'No leanck installer state found at %s; nothing to remove.\n' "$state" >&2
        exit 0
    fi
    die "no leanck executable with _uninstall found; download a leanck release archive, extract it, and run its uninstall.sh"
}

main "$@"
