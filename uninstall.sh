#!/bin/sh
# Standalone, offline uninstaller. Keep execution in main for curl | sh safety.

say() { printf '%s\n' "$*" >&2; }
die() { say "leanck-uninstall: $*"; exit 1; }
usage() {
    cat <<'EOF'
Usage: sh uninstall.sh [OPTIONS]

Remove installer-managed leanck files and host plugin registrations.
Modified files, user configuration, Lean projects, caches, and installer backups
are preserved. The executable is used to stop leanck daemons before removal.

  -y, --yes          Approve removal without prompting
  --dry-run          Preview only; never run leanck or host commands
  --state-dir DIR    Installer state (default: $XDG_STATE_HOME/leanck/installer,
                     or ~/.local/state/leanck/installer)
  --plugin HOSTS     Also unregister claude, codex, or both (for older installs)
  --timeout SECONDS  Maximum time for each external command (default: 30)
  -h, --help         Show this help

Examples:
  sh uninstall.sh --dry-run
  sh uninstall.sh --yes
  sh uninstall.sh --state-dir '/custom state/leanck/installer'
EOF
}
valid_path() {
    case "$1" in /*) ;; *) die "expected an absolute path: $1" ;; esac
    case "$1/" in //|*/../*) die "unsafe path: $1" ;; esac
    if printf '%s' "$1" | LC_ALL=C grep '[[:cntrl:]]' >/dev/null; then die 'control characters in path'; fi
}
safe_parents() {
    ancestor=$(dirname "$1")
    while [ "$ancestor" != / ]; do
        if [ -L "$ancestor" ]; then
            # macOS exposes these system directories through root-owned aliases.
            case "$ancestor:$(readlink "$ancestor")" in
                /var:private/var|/var:/private/var|/tmp:private/tmp|/tmp:/private/tmp) ;;
                *) return 1 ;;
            esac
        fi
        ancestor=$(dirname "$ancestor")
    done
}
digest_file() {
    if [ "$hash_command" = sha256sum ]; then sha256sum < "$1"; else shasum -a 256 < "$1"; fi | awk '{print $1}'
}
tree_digest() {
    (cd "$1" && find . -type f ! -path ./.leanck-content-sha256 | LC_ALL=C sort | while IFS= read -r file; do
        printf '%s %s\n' "$(digest_file "$file")" "$file"
    done) > "$work/tree-digest"
    digest_file "$work/tree-digest"
}
record() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$work/inventory"; }
legacy_tree() {
    [ ! -L "$1" ] && [ -f "$1/.leanck-content-sha256" ] || return 0
    record tree "$(cat "$1/.leanck-content-sha256")" "$1"
}
legacy_agents() {
    for marker in "$1"/*.leanck-installed; do
        [ -f "$marker" ] && [ ! -L "$marker" ] || continue
        target=${marker%.leanck-installed}
        [ -f "$target.leanck-sha256" ] || continue
        if [ -L "$target" ] && [ "$(readlink "$target")" = "$data_dir/agents/${target##*/}" ]; then
            record link "$data_dir/agents/${target##*/}" "$target"
        else record agent "$(cat "$target.leanck-sha256")" "$target"; fi
    done
}
legacy_inventory() {
    # Selections are data, never shell code. Only older installers need discovery.
    data_dir=${XDG_DATA_HOME:-$HOME/.local/share}/leanck
    claude_dir=${CLAUDE_CONFIG_DIR:-$HOME/.claude}
    codex_dir=${CODEX_HOME:-$HOME/.codex}
    codex_skills=$HOME/.agents/skills codex_agents_dir=''
    path_file='' last_binary_path='' last_binary_digest=''
    last_completions_path='' last_completions_digest=''
    while IFS="$tab" read -r key value; do
        case "$key" in
            data_dir|claude_dir|codex_dir|codex_skills|codex_agents_dir|path_file|last_binary_path|last_binary_digest|last_completions_path|last_completions_digest)
                eval "$key=\$value" ;;
            extra_skills)
                valid_path "$value"
                for skill in "$value"/*; do legacy_tree "$skill"; done ;;
            extra_claude_agents|extra_codex_agents) valid_path "$value"; legacy_agents "$value" ;;
        esac
    done < "$state_dir/selections.tsv"
    for directory in "$data_dir" "$claude_dir" "$codex_dir" "$codex_skills" "${codex_agents_dir:-$codex_dir/agents/leanck}"; do
        valid_path "$directory"
    done
    for directory in "$claude_dir/skills" "$codex_skills"; do
        for skill in "$directory"/*; do legacy_tree "$skill"; done
    done
    legacy_agents "$claude_dir/agents"
    legacy_agents "$codex_dir/agents"
    legacy_agents "${codex_agents_dir:-$codex_dir/agents/leanck}"
    legacy_tree "$data_dir/agents"
    legacy_tree "$data_dir/bundle"
    [ -z "$last_binary_path" ] || record file "$last_binary_digest" "$last_binary_path"
    [ -z "$last_completions_path" ] || record file "$last_completions_digest" "$last_completions_path"
    [ -z "$path_file" ] || record profile - "$path_file"
    if [ -d "$data_dir/bundle" ] && [ "$plugin" = none ]; then
        die 'older installer did not record plugin hosts; rerun with --plugin claude, codex, or both to remove registrations'
    fi
}
unchanged() {
    safe_parents "$path" || return 1
    case "$kind" in
        tree)
            [ ! -L "$path" ] && [ -d "$path" ] &&
                [ -f "$path/.leanck-installed" ] && [ ! -L "$path/.leanck-installed" ] &&
                [ "$(cat "$path/.leanck-installed")" = 'leanck installer v1' ] || return 1
            # Reject links/special files, which the installer's file hash excludes.
            [ -z "$(find "$path" ! -type f ! -type d -print)" ] || return 1
            [ "$(tree_digest "$path")" = "$expected" ] ;;
        file|agent)
            [ ! -L "$path" ] && [ -f "$path" ] && [ "$(digest_file "$path")" = "$expected" ] ;;
        link) [ -L "$path" ] && [ "$(readlink "$path")" = "$expected" ] ;;
        profile)
            [ ! -L "$path" ] && [ -f "$path" ] || return 1
            awk '
                /^# >>> leanck PATH >>>$/ {if(open || count) bad=1; open=1; count++; next}
                /^# <<< leanck PATH <<<$/ {if(!open) bad=1; open=0}
                END {exit (bad || open)}
            ' "$path" ;;
        host|marketplace) return 0 ;;
    esac
}
run_bounded() {
    "$@" </dev/null >&2 & child=$!
    (
        sleeper=''
        trap '[ -z "$sleeper" ] || kill "$sleeper" 2>/dev/null; exit 0' TERM INT HUP
        sleep "$timeout" & sleeper=$!; wait "$sleeper" || exit 0
        say "Command timed out: $1"
        kill -TERM "$child" 2>/dev/null || exit 0
        sleep 1 & sleeper=$!; wait "$sleeper" || exit 0
        kill -KILL "$child" 2>/dev/null || :
    ) & watchdog=$!
    if wait "$child"; then status=0; else status=$?; fi
    child=''
    kill "$watchdog" 2>/dev/null || :
    wait "$watchdog" 2>/dev/null || :
    watchdog=''
    return "$status"
}
cleanup() {
    [ -z "$child" ] || kill -TERM "$child" 2>/dev/null || :
    [ -z "$watchdog" ] || kill "$watchdog" 2>/dev/null || :
    [ -z "$pending" ] || rm -f "$pending"
    [ -z "$work" ] || rm -rf "$work"
}
save_remaining() {
    pending=$(mktemp "$state_dir/.leanck-uninstall.XXXXXX")
    cat "$work/inventory" > "$pending"
    mv -f "$pending" "$state_dir/uninstall.tsv"
    pending=''
}
forget_record() {
    LEANCK_REMOVE_KIND=$kind LEANCK_REMOVE_EXPECTED=$expected LEANCK_REMOVE_PATH=$path \
        awk -F '\t' '!($1==ENVIRON["LEANCK_REMOVE_KIND"] && $2==ENVIRON["LEANCK_REMOVE_EXPECTED"] && $3==ENVIRON["LEANCK_REMOVE_PATH"])' \
        "$work/inventory" > "$work/remaining"
    cat "$work/remaining" > "$work/inventory"
    save_remaining
}
remove_record() {
    if [ "$kind" = host ] || [ "$kind" = marketplace ]; then
        command -v "$expected" >/dev/null 2>&1 || die "$expected is unavailable; restore it to unregister the plugin, then rerun"
        if [ "$expected" = claude ]; then
            if [ "$kind" = host ]; then
                run_bounded env CLAUDE_CONFIG_DIR="$path" claude plugin uninstall leanck@leanck-installer --scope user || return 1
            else
                run_bounded env CLAUDE_CONFIG_DIR="$path" claude plugin marketplace remove leanck-installer --scope user || return 1
            fi
        else
            if [ "$kind" = host ]; then
                run_bounded env CODEX_HOME="$path" codex plugin remove leanck@leanck-installer || return 1
            else
                run_bounded env CODEX_HOME="$path" codex plugin marketplace remove leanck-installer || return 1
            fi
        fi
    elif [ -e "$path" ] || [ -L "$path" ]; then
        unchanged || { say "Preserved changed or unsafe path: $path"; return 0; }
        case "$kind" in
            profile)
                pending=$(mktemp "$(dirname "$path")/.leanck-uninstall.XXXXXX")
                cp -p "$path" "$pending" || return 1
                awk '/^# >>> leanck PATH >>>$/ {skip=1;next} /^# <<< leanck PATH <<<$/ {skip=0;next} !skip' "$path" > "$pending" || return 1
                mv -f "$pending" "$path" || return 1
                pending='' ;;
            tree) rm -rf "$path" || return 1 ;;
            file) rm -f "$path" || return 1 ;;
            agent|link)
                rm -f "$path" || return 1
                # Sidecars belong to this agent; never recursively remove the host directory.
                for suffix in leanck-installed leanck-sha256; do
                    if [ ! -d "$path.$suffix" ]; then rm -f "$path.$suffix" || return 1; fi
                done ;;
        esac
        say "Removed $kind: $path"
    fi
    forget_record
}
main() {
    set -eu
    umask 077
    yes=0 dry_run=0 timeout=30 plugin=none work='' pending='' child='' watchdog=''
    state_dir=${XDG_STATE_HOME:-${HOME:?HOME must be set}/.local/state}/leanck/installer
    while [ "$#" -gt 0 ]; do
        case "$1" in
            -y|--yes) yes=1 ;;
            --dry-run) dry_run=1 ;;
            -h|--help) usage; return 0 ;;
            --state-dir|--timeout|--plugin)
                [ "$#" -ge 2 ] || die "$1 requires a value"
                case "$1" in --state-dir) state_dir=$2 ;; --timeout) timeout=$2 ;; --plugin) plugin=$2 ;; esac
                shift ;;
            *) die "unknown option: $1 (see --help)" ;;
        esac
        shift
    done
    case "$timeout" in ''|*[!0-9]*|0) die '--timeout expects a positive integer' ;; esac
    case "$plugin" in none|claude|codex|both) ;; *) die '--plugin expects claude, codex, or both' ;; esac
    case "$state_dir" in /*) ;; *) state_dir=$PWD/$state_dir ;; esac
    valid_path "$state_dir"
    safe_parents "$state_dir/selections.tsv" || die 'refusing symlink installer state directory'
    for name in selections.tsv uninstall.tsv; do
        [ ! -L "$state_dir/$name" ] || die "refusing symlink: $state_dir/$name"
    done
    if [ ! -f "$state_dir/uninstall.tsv" ] && [ ! -f "$state_dir/selections.tsv" ]; then
        say "No leanck installer state found at $state_dir; nothing to remove."
        return 0
    fi
    if command -v sha256sum >/dev/null 2>&1; then hash_command=sha256sum
    elif command -v shasum >/dev/null 2>&1; then hash_command=shasum
    else die 'sha256sum or shasum is required'; fi
    work=$(mktemp -d "${TMPDIR:-/tmp}/leanck-uninstall.XXXXXX")
    trap cleanup 0
    trap 'exit 130' INT
    trap 'exit 143' TERM HUP
    tab=$(printf '\t')
    : > "$work/inventory"
    if [ -f "$state_dir/uninstall.tsv" ]; then cat "$state_dir/uninstall.tsv" > "$work/inventory"
    else legacy_inventory; fi
    if [ "$plugin" != none ]; then
        [ "$plugin" = codex ] || record host claude "${claude_dir:-${CLAUDE_CONFIG_DIR:-$HOME/.claude}}"
        [ "$plugin" = claude ] || record host codex "${codex_dir:-${CODEX_HOME:-$HOME/.codex}}"
    fi
    # Host removal and marketplace removal have separate receipts for safe retries.
    awk -F '\t' '!seen[$0]++ {print; if($1=="host") print "marketplace\t" $2 "\t" $3}' "$work/inventory" > "$work/expanded"
    awk '!seen[$0]++' "$work/expanded" > "$work/inventory"
    # Unregister hosts before deleting their source; unlink agents before source trees.
    awk -F '\t' '{rank=($1=="host" ? 0 : $1=="marketplace" ? 1 : $1=="tree" ? 3 : 2); print rank "\t" $0}' "$work/inventory" |
        LC_ALL=C sort -n | cut -f 2- > "$work/ordered"
    while IFS="$tab" read -r kind expected path extra; do
        [ -z "$extra" ] || die 'malformed uninstall inventory'
        valid_path "$path"
        case "$kind" in
            host|marketplace) case "$expected" in claude|codex) ;; *) die 'invalid host receipt' ;; esac ;;
            tree|file|agent|link|profile) ;;
            *) die "invalid uninstall inventory kind: $kind" ;;
        esac
        if [ "$kind" != host ] && [ "$kind" != marketplace ] && { [ -e "$path" ] || [ -L "$path" ]; } && ! unchanged; then
            say "Preserve changed or unsafe path: $path"
        else
            say "Would remove $kind: $path"
            if [ "$kind" = file ] && [ "${path##*/}" = leanck ] && [ -x "$path" ]; then
                say "Would stop leanck daemons using $path daemons stop --all"
            fi
        fi
    done < "$work/ordered"
    [ "$dry_run" = 0 ] || { say 'Dry run complete; no installed files or host configuration changed.'; return 0; }
    if [ "$yes" = 0 ]; then
        if ! { exec 3<>/dev/tty; } 2>/dev/null; then die 'use --yes to approve removal without a terminal'; fi
        printf 'Uninstall leanck? [y/N]: ' >&2
        IFS= read -r answer <&3 || exit 130
        case "$answer" in y|Y|yes|YES) ;; *) say 'Cancelled.'; return 0 ;; esac
    fi
    save_remaining
    # Stop before any removal, but only execute an unchanged, recorded binary.
    while IFS="$tab" read -r kind expected path; do
        if [ "$kind" = file ] && [ "${path##*/}" = leanck ] && [ -x "$path" ] && unchanged; then
            run_bounded "$path" daemons stop --all || die 'could not stop leanck daemons; files preserved'
            break
        fi
    done < "$work/ordered"
    while IFS="$tab" read -r kind expected path; do
        remove_record || die 'removal failed; remaining files and receipts preserved; rerun to resume'
    done < "$work/ordered"
    if [ -s "$work/inventory" ]; then
        say "Uninstall incomplete: edited files were preserved. Remaining paths are recorded in $state_dir/uninstall.tsv."
        return 1
    fi
    rm -f "$state_dir/uninstall.tsv" "$state_dir/selections.tsv"
    say "Uninstalled leanck. Installer backups remain in $state_dir/backups. Restart your agent hosts and shell."
}

main "$@"
