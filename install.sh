#!/bin/sh
# Install a published leanck release. Keep this script usable with curl | sh.
# All execution is inside main so a truncated download cannot start installation.

# Prompts use a separate terminal descriptor: stdin may contain this script.
INSTALLER_VERSION="0.3.0"
colors() {
    bold='' cyan='' green='' yellow='' red='' reset='' out_bold='' out_reset=''
    prompt_style='' selected_style='' input_style=''
    if [ "$no_color" = 0 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != dumb ] && [ -t 1 ]; then
        out_bold=$(printf '\033[1m'); out_reset=$(printf '\033[0m')
    fi
    if [ "$no_color" = 0 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != dumb ] && [ -t 2 ]; then
        bold=$(printf '\033[1m'); cyan=$(printf '\033[36m'); green=$(printf '\033[32m')
        yellow=$(printf '\033[33m'); red=$(printf '\033[31m'); reset=$(printf '\033[0m')
        prompt_style=$bold$cyan; selected_style=$bold$green; input_style=$reset
    fi
}
say() { if [ "${quiet:-0}" = 0 ]; then printf '%s[INFO]%s %s\n' "${cyan:-}" "${reset:-}" "$*" >&2; fi; }
warn() { printf '%s[WARN]%s %s\n' "${yellow:-}" "${reset:-}" "$*" >&2; }
die() {
    error=$*
    printf '%s [ERR]:%s %s\n' "${red:-}" "${reset:-}" "$error" >&2
    exit 1
}
json_string() {
    # No Python/jq dependency. Preserve UTF-8 and escape every JSON control byte.
    printf '%s' "$1" | LC_ALL=C awk '
    BEGIN { printf "\""; for (i=1;i<32;i++) escapes[sprintf("%c",i)]=sprintf("\\u%04x",i) }
    { if (NR>1) printf "\\n"; for(i=1;i<=length($0);i++) {
        c=substr($0,i,1); if(c=="\"") printf "\\\""; else if(c=="\\") printf "\\\\";
        else if(c in escapes) printf "%s",escapes[c]; else printf "%s",c
    }} END { printf "\"" }'
}
emit_json() {
    [ "$json" = 1 ] || return 0
    printf '{"status":'; json_string "$outcome"
    printf ',"version":'; json_string "${installed_version:-$version}"
    printf ',"message":'; json_string "$error"
    printf ',"actions":['
    separator=''
    if [ -n "$work" ] && [ -f "$work/plan" ]; then
        while IFS= read -r action; do
            printf '%s' "$separator"; json_string "$action"; separator=,
        done < "$work/plan"
    fi
    printf ']}\n'
}
valid_text() {
    # Reject multiline/control input rather than putting pasted commands in profiles/state.
    if printf '%s' "$1" | LC_ALL=C grep '[[:cntrl:]]' >/dev/null 2>&1; then
        die 'values must be single-line text without control characters'
    fi
    case "$1" in *"
"*) die 'values must be single-line text' ;; esac
}
prompt() {
    printf '%s%s%s [%s]: %s' "$prompt_style" "$1" "$reset" "$2" "$input_style" >&2
    if [ "$3" = text ]; then
        # Bash readline supplies cursor editing, history editing, and filename completion.
        # No content is evaluated; fd 3 is the controlling terminal, never the script pipe.
        reply=$(bash -c 'bind "set enable-bracketed-paste on" 2>/dev/null || :; IFS= read -e -r answer <&3 || exit 1; printf "%s" "$answer"') || cancel
    else
        IFS= read -r reply <&3 || cancel
    fi
    printf '%s' "$reset" >&2
    valid_text "$reply"
    reply=${reply:-$2}
}
yes_no() {
    while :; do
        prompt "$1" "$2" choice
        case "$reply" in y|Y|yes|YES) answer=1; return ;; n|N|no|NO) answer=0; return ;; esac
        warn 'Enter yes or no.'
    done
}
cancel() { outcome=cancelled; say 'Cancelled; no further changes will be made.'; exit 130; }
choose_hosts() {
    while :; do
        prompt "$2 (none / claude / codex / both)" "$3" choice
        case "$reply" in none|claude|codex|both) break ;; esac
        warn 'Choose none, claude, codex, or both.'
    done
    eval "${1}_claude=0; ${1}_codex=0"
    [ "$reply" = none ] || select_hosts "$1" "$reply"
}
host_default() {
    case "$1$2" in 00) host_choice=none ;; 10) host_choice=claude ;; 01) host_choice=codex ;; 11) host_choice=both ;; esac
}
choose() {
    # Bash reads individual keys from the controlling terminal, never the script pipe.
    # Restore terminal settings on every exit, including Ctrl-C and a closed terminal.
    if [ "${TERM:-}" = dumb ]; then
        while :; do
            prompt "$1 ($4)" "$2" choice
            case " $3 " in *" $reply "*) return ;; esac
            warn "Choose $4."
        done
    fi
    reply=$(bash -c '
        title=$1 selected=$2 values=$3 labels=$4
        prompt_style=$5 selected_style=$6 reset_style=$7
        IFS=" " read -r -a choices <<< "$values"
        IFS="|" read -r -a names <<< "$labels"
        index=0
        for ((i=0; i<${#choices[@]}; i++)); do
            [[ ${choices[i]} != "$selected" ]] || index=$i
        done
        saved=$(stty -g <&3) || exit 1
        trap '\''printf "%s\033[?25h" "$reset_style" >&2; stty "$saved" <&3'\'' EXIT
        trap '\''exit 130'\'' INT TERM HUP
        stty -echo -icanon -isig min 1 time 0 <&3 || exit 1
        printf "\033[?25l" >&2
        redraw=0
        while :; do
            if ((redraw)); then printf "%s\r\033[%sA" "$reset_style" "${#choices[@]}" >&2
            else printf "\n%s%s%s\n" "$prompt_style" "$title" "$reset_style" >&2; fi
            for ((i=0; i<${#choices[@]}; i++)); do
                marker="  " style=$reset_style
                if ((i == index)); then marker="> "; style=$selected_style; fi
                printf "\r\033[2K%s%s%s%s\n" "$style" "$marker" "${names[i]}" "$reset_style" >&2
            done
            printf "\r\033[2K" >&2
            redraw=1
            while :; do
                IFS= read -r -s -n 1 key <&3 || exit 130
                case "$key" in
                    $'\''\003'\''|$'\''\004'\'') exit 130 ;;
                    "")
                        printf "%s\n" "$reset_style" >&2
                        printf "%s" "${choices[index]}"
                        exit 0 ;;
                    $'\''\033'\'')
                        IFS= read -r -s -n 1 -t 1 key <&3 || continue
                        [[ $key == "[" || $key == O ]] || continue
                        IFS= read -r -s -n 1 -t 1 key <&3 || continue
                        case "$key" in
                            A) index=$(((index + ${#choices[@]} - 1) % ${#choices[@]})) ;;
                            B) index=$(((index + 1) % ${#choices[@]})) ;;
                            *) continue ;;
                        esac
                        break ;;
                    *) ;; # Text input is ignored while the menu cursor is hidden.
                esac
            done
        done
    ' installer-menu "$1" "$2" "$3" "$4" "$prompt_style" "$selected_style" "$reset") || cancel
}
detect_hosts() {
    detected_claude=0 detected_codex=0 detected_agents=''
    if command -v claude >/dev/null 2>&1; then detected_claude=1; detected_agents='Claude Code'; fi
    if command -v codex >/dev/null 2>&1; then
        detected_codex=1; detected_agents=${detected_agents:+$detected_agents, }Codex
    fi
    detected_agents=${detected_agents:-none}
}
configure_standalone() {
    yes_no 'Install the CLI?' "$( [ "$cli" = 1 ] && printf yes || printf no )"; cli=$answer
    if [ "$cli" = 1 ]; then prompt 'Install location' "$bin_dir" text; bin_dir=$reply; fi
    host_default "$skills_claude" "$skills_codex"; choose_hosts skills 'Standalone skills' "$host_choice"
    host_default "$agents_claude" "$agents_codex"; choose_hosts agents 'Custom agents' "$host_choice"
    if [ "$skills_claude$agents_claude" != 00 ]; then prompt 'Claude configuration directory' "$claude_dir" text; claude_dir=$reply; fi
    if [ "$skills_codex" = 1 ]; then prompt 'Codex skills directory' "$codex_skills" text; codex_skills=$reply; fi
    if [ "$agents_codex" = 1 ]; then prompt 'Codex configuration directory' "$codex_dir" text; codex_dir=$reply; fi
    if [ "$cli" = 1 ]; then
        configure_completions
        case ":$original_path:" in *":$bin_dir:"*) ;; *)
            yes_no 'Add leanck to PATH?' "$( [ "$add_to_path" = 1 ] && printf yes || printf no )"
            add_to_path=$answer ;;
        esac
    fi
}
absolute_path() {
    case "$1" in
        '~') resolved=$HOME ;;
        '~/'*) resolved=$HOME/${1#??} ;;
        /*) resolved=$1 ;;
        *) resolved=$PWD/$1 ;;
    esac
    valid_text "$resolved"
}
default_completions() {
    case "$1" in
        bash|zsh|fish|elvish) completions=$1 ;;
        pwsh|powershell) completions=powershell ;;
        *) completions=none ;;
    esac
}
completion_default_dir() {
    case "$1" in
        fish) printf '%s' "$fish_completions_dir" ;;
        bash) printf '%s/bash-completion/completions' "${XDG_DATA_HOME:-$HOME/.local/share}" ;;
        zsh) printf '%s/zsh/site-functions' "${XDG_DATA_HOME:-$HOME/.local/share}" ;;
        powershell) printf '%s/powershell/Completions' "${XDG_CONFIG_HOME:-$HOME/.config}" ;;
        elvish) printf '%s/elvish/lib' "${XDG_CONFIG_HOME:-$HOME/.config}" ;;
    esac
}
completion_destination() {
    completion_dir=${completions_dir:-$(completion_default_dir "$completions")}
    case "$completions" in
        fish) completion_name=leanck.fish ;; bash) completion_name=leanck ;;
        zsh) completion_name=_leanck ;; powershell) completion_name=leanck.ps1 ;;
        elvish) completion_name=leanck.elv ;; *) completion_name='' ;;
    esac
    completion_path=$completion_dir/$completion_name
}
configure_completions() {
    while :; do
        prompt 'Shell completions (none / bash / zsh / fish / powershell / elvish)' "$completions" choice
        case "$reply" in none|bash|zsh|fish|powershell|elvish) completions=$reply; break ;; esac
        warn 'Choose none, bash, zsh, fish, powershell, or elvish.'
    done
    if [ "${1:-}" = advanced ] && [ "$completions" != none ]; then
        completion_destination
        prompt 'Completions directory' "$completion_dir" text
        absolute_path "$reply"; completions_dir=$resolved
    fi
}
configure_path() {
    yes_no 'Add leanck to PATH?' "$( [ "$add_to_path" = 1 ] && printf yes || printf no )"
    add_to_path=$answer
    [ "$add_to_path" = 1 ] || return 0
    case "$shell" in fish|bash|zsh) ;;
        *)
            while :; do
                prompt 'Shell for PATH (fish / bash / zsh)' bash choice
                case "$reply" in fish|bash|zsh) shell=$reply; break ;; esac
                warn 'Choose fish, bash, or zsh.'
            done
            if [ "$profile_set" = 0 ]; then
                case "$shell" in
                    fish) path_file=${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish ;;
                    zsh) path_file=$HOME/.zshrc ;;
                    bash) path_file=$HOME/.bashrc ;;
                esac
            fi ;;
    esac
}
codex_agents_destination() { codex_agent_path=${codex_agents_dir:-$codex_dir/agents/leanck}; }
has_components() {
    [ "$cli$skills_claude$skills_codex$agents_claude$agents_codex" != 00000 ] ||
        [ -n "$extra_skills$extra_claude_agents$extra_codex_agents" ]
}
append_location() {
    location_key=$1
    case "$location_key" in extra_skills|extra_claude_agents|extra_codex_agents) ;; *) die 'invalid additional location' ;; esac
    absolute_path "$2"
    eval 'locations=$'"$location_key"
    # Lists are newline-delimited internally; each saved TSV record contains one path.
    case "
$locations
" in *"
$resolved
"*) return ;; esac
    locations=${locations:+$locations
}$resolved
    eval "$location_key=\$locations"
}
write_location_values() (
    for key in extra_skills extra_claude_agents extra_codex_agents; do
        eval 'locations=$'"$key"
        [ -n "$locations" ] || continue
        printf '%s\n' "$locations" | while IFS= read -r location; do printf '%s\t%s\n' "$key" "$location"; done
    done
)
extra_locations() {
    extra_action=$1
    for extra_key in extra_skills extra_claude_agents extra_codex_agents; do
        eval 'extra_paths=$'"$extra_key"
        [ -n "$extra_paths" ] || continue
        while IFS= read -r extra_location; do
            [ -n "$extra_location" ] || continue
            case "$extra_key" in
                extra_skills)
                    for extra_skill in "$plugin"/skills/*; do
                        if [ "$extra_action" = check ]; then check_destination "$extra_location/${extra_skill##*/}"
                        else install_tree "$extra_skill" "$extra_location/${extra_skill##*/}"; fi
                    done ;;
                extra_claude_agents)
                    if [ "$extra_action" = check ]; then check_agents "$plugin/agents" "$extra_location" '*.md'
                    else install_agents "$plugin/agents" "$extra_location" '*.md'; fi ;;
                extra_codex_agents)
                    if [ "$extra_action" = check ]; then check_agents "$plugin/codex/agents" "$extra_location" '*.toml'
                    else install_agents "$plugin/codex/agents" "$extra_location" '*.toml'; fi ;;
            esac
        done <<EOF
$extra_paths
EOF
    done
}

plan() { printf '%s\n' "$*" >> "$work/plan"; }
warn_skipped_option() {
    printf '%s\n' "$*" >> "$work/warnings"
    if [ "$interactive" = 0 ]; then warn "$*"; fi
}
# Keep a cumulative uninstall inventory, including destinations from older installs.
record_install() {
    inventory_kind=$1; [ "$inventory_kind" != completion ] || inventory_kind=file
    printf '%s\t%s\t%s\n' "$inventory_kind" "$2" "$3" >> "$work/uninstall.tsv"
    if [ -n "${4:-}" ]; then
        printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" >> "$work/sources.tsv"
    fi
}
preview() { [ "$dry_run" = 1 ] || [ "$planning" = 1 ]; }
digest_file() {
    if [ "$hash_command" = sha256sum ]; then sha256sum < "$1"; else shasum -a 256 < "$1"; fi | awk '{print $1}'
}
tree_digest() {
    (cd "$1" && find . -type f ! -path ./.leanck-content-sha256 ! -name .leanck-installation.json | LC_ALL=C sort | while IFS= read -r file; do
        printf '%s %s\n' "$(digest_file "$file")" "$file"
    done) > "$work/tree-digest"
    digest_file "$work/tree-digest"
}
conflict() {
    if [ "$force" = 1 ]; then plan "Back up and replace conflict: $1"; return; fi
    if [ "$interactive" = 1 ]; then
        warn "$1 already exists or has local edits."
        while :; do
            prompt 'Keep it, back up and replace it, or cancel? (keep / replace / cancel)' keep choice
            case "$reply" in
                keep) printf '%s\n' "$1" >> "$work/skip"; plan "Preserve $1"; return ;;
                replace) plan "Back up and replace conflict: $1"; return ;;
                cancel) cancel ;;
            esac
        done
    fi
    die "$1 already exists or has local edits; use --force to back up and replace it, or rerun interactively"
}
skipped() { grep -F -x -e "$1" "$work/skip" >/dev/null 2>&1; }
commit() {
    # Each replacement has a persistent backup and a reverse-ordered rollback record.
    destination=$2
    [ ! -L "$(dirname "$destination")" ] || die "refusing symlink parent: $destination"
    mkdir -p "$(dirname "$destination")"
    backup='-'
    if [ -e "$destination" ] || [ -L "$destination" ]; then
        backup=$backup_dir/$sequence
        mkdir -p "$backup_dir"
        cp -p -R -P "$destination" "$backup"
        printf '%s\t%s\n' "$destination" "$backup" >> "$backup_dir/manifest.tsv"
    fi
    printf '%s\t%s\n' "$destination" "$backup" >> "$work/journal"
    sequence=$((sequence + 1))
    # A "-" source retires an obsolete managed file through the same transaction.
    if [ "$1" = '-' ]; then
        rm -f "$destination"
        return
    fi
    # Trees cannot replace nonempty directories with rename. Move the old tree aside
    # on the same filesystem until the new tree has been committed.
    if [ -L "$destination" ]; then
        rm "$destination"
        mv -f "$1" "$destination"
    elif [ -d "$destination" ]; then
        previous=$destination.leanck-previous-$$
        [ ! -e "$previous" ] || die "temporary destination exists: $previous"
        mv "$destination" "$previous"
        if ! mv -f "$1" "$destination"; then mv "$previous" "$destination"; die "could not install $destination"; fi
        rm -rf "$previous"
    else
        mv -f "$1" "$destination"
    fi
    pending=''
}
# Bound external commands without depending on GNU timeout (absent on macOS).
# The watchdog's trap also reaps its sleep when a fast command finishes.
run_bounded() {
    command_limit=$1; shift
    "$@" &
    active_child=$!
    (
        sleeper=''
        trap '[ -z "$sleeper" ] || kill "$sleeper" 2>/dev/null; exit 0' TERM INT HUP
        sleep "$command_limit" & sleeper=$!
        wait "$sleeper" || exit 0
        printf 'Command timed out after %s seconds: %s\n' "$command_limit" "$1" >&2
        kill -TERM "$active_child" 2>/dev/null || exit 0
        sleep 1 & sleeper=$!
        wait "$sleeper" || exit 0
        kill -KILL "$active_child" 2>/dev/null || :
    ) &
    command_watchdog=$!
    if wait "$active_child"; then command_status=0; else command_status=$?; fi
    active_child=''
    kill "$command_watchdog" 2>/dev/null || :
    wait "$command_watchdog" 2>/dev/null || :
    command_watchdog=''
    return "$command_status"
}
stop_command() {
    if [ -n "$active_child" ]; then
        kill -TERM "$active_child" 2>/dev/null || :
        kill -KILL "$active_child" 2>/dev/null || :
        wait "$active_child" 2>/dev/null || :
        active_child=''
    fi
    if [ -n "$command_watchdog" ]; then
        kill "$command_watchdog" 2>/dev/null || :
        wait "$command_watchdog" 2>/dev/null || :
        command_watchdog=''
    fi
}
rollback() {
    [ -s "$work/journal" ] || return 0
    warn 'Restoring replaced files; press Ctrl-C again to exit immediately. Backups remain available.'
    trap 'exit 130' INT TERM HUP
    awk '{line[NR]=$0} END {for(i=NR;i>0;i--) print line[i]}' "$work/journal" > "$work/rollback"
    tab=$(printf '\t')
    while IFS="$tab" read -r destination backup; do
        if ! run_bounded 10 rm -rf "$destination"; then
            warn "Could not remove $destination during rollback; use the backups in $backup_dir"; continue
        fi
        if [ "$backup" != '-' ]; then
            run_bounded 10 cp -p -R -P "$backup" "$destination" || warn "Restore $destination from $backup"
        fi
    done < "$work/rollback"
}


usage() {
    b=$out_bold r=$out_reset
    cat <<EOF
${b}Install leanck${r} — configure, review, and approve changes to your user account.

${b}Usage:${r} sh install.sh [OPTIONS]

${b}Common options:${r}
  -y, --yes                        Approve selected settings without prompts (required without a TTY)
      --dry-run                    Verify and preview without executing code or modifying files
      --force                      Back up and replace conflicting files (does not imply --yes)
      --plugin HOST                Register plugins: claude, codex, both, or none (default: detected)
      --all                        Plugin mode for both hosts plus Codex custom agents
      --standalone                 Install standalone components instead of the plugin
      --version VERSION            Release tag to install (default: latest, or saved selection)
      --from-repo DIR              Build and install from a local checkout (skipped on --dry-run)
  -h, --help                       Show this help (also: sh install.sh help)

${b}Component selection:${r}
      --cli, --no-cli              Install or skip the executable (default: --cli)
      --skills HOST                Standalone skills: none, claude, codex, or both
      --agents HOST                Custom agents: none, claude, codex, or both
      --completions SHELL          Shell completions: bash, zsh, fish, powershell, elvish, or none

${b}Directories & paths:${r}
      --bin-dir DIR                Executable dir (plugin: DATA_DIR/bin; standalone: ~/.local/bin)
      --data-dir DIR               Plugin data dir (default: \$XDG_DATA_HOME/leanck)
      --state-dir DIR              Installer state & backups (default: \$XDG_STATE_HOME/leanck/installer)
      --claude-dir DIR             Claude config dir (default: \$CLAUDE_CONFIG_DIR or ~/.claude)
      --codex-dir DIR              Codex config dir (default: \$CODEX_HOME or ~/.codex)
      --codex-skills-dir DIR       Codex standalone skills dir (default: ~/.agents/skills)
      --codex-agents-dir DIR       Codex agent definitions dir (default: CODEX_DIR/agents/leanck)
      --completions-dir DIR        Override the selected shell's completions directory
      --fish-completions-dir DIR   Fish completions default dir (honors XDG_CONFIG_HOME)
      --extra-skills-dir DIR       Additional skills directory (repeatable)
      --extra-claude-agents-dir DIR  Additional Claude agent directory (repeatable)
      --extra-codex-agents-dir DIR   Additional Codex agent directory (repeatable)
      --shell SHELL                Shell for PATH updates: fish, bash, or zsh (default: \$SHELL)
      --add-to-path                Add a managed PATH block to the selected shell profile
      --no-add-to-path             Disable a remembered PATH update
      --path-file FILE             Override the shell profile file to update

${b}Source & output:${r}
      --release-url URL            HTTPS release mirror (default: GitHub releases)
      --timeout SECONDS            Download and command timeout in seconds (default: 300)
      --json                       Emit JSON status/errors; disables prompts (use --yes to install)
      --no-color                   Disable ANSI color (also honors NO_COLOR and TERM=dumb)
      --no-pager                   Print long plans without paging
  -q, --quiet                      Suppress non-essential progress messages
  -v, --verbose                    Increase detail (repeatable for command output)

${b}Examples:${r}
  curl -fsSL https://lean4.party/leanck-install.sh | sh
  sh install.sh --plugin codex
  sh install.sh --standalone --skills both
  sh install.sh --yes --all --version v0.30.0
  sh install.sh --from-repo .
  sh install.sh --dry-run --json
EOF
}

need_value() {
    [ "$#" -ge 2 ] && [ -n "$2" ] || die "$1 needs a value; see --help"
    case "$2" in --*) die "$1 needs a value; see --help" ;; esac
}

select_hosts() {
    case "$2" in
        none) ;;
        claude) eval "${1}_claude=1" ;;
        codex) eval "${1}_codex=1" ;;
        both) eval "${1}_claude=1; ${1}_codex=1" ;;
        *) die "$1: expected none, claude, codex, or both (got $2)" ;;
    esac
}

download() {
    say "Downloading ${1##*/}"
    [ "$verbose" -lt 1 ] || say "Source: $1"
    progress_pid=''
    if [ -t 1 ] && [ -t 2 ] && [ "$quiet" = 0 ] && [ "$json" = 0 ] && [ "${TERM:-}" != dumb ]; then
        (trap 'exit 0' INT TERM HUP; while :; do
            for frame in '|' '/' '-' '\\'; do printf '\r%s %s' "$frame" 'Downloading…' >&2; sleep 0.2; done
        done) &
        progress_pid=$!
    fi
    if http_status=$(curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
        --tlsv1.2 --connect-timeout 15 --max-time "$timeout" --retry 2 \
        --write-out '%{http_code}' --output "$2" "$1" 2> "$2.error"); then
        download_exit=0
    else
        download_exit=$?
    fi
    stop_progress
    [ "$download_exit" != 0 ] || return 0
    if [ "$dry_run" = 1 ] && [ "$http_status" = 404 ]; then
        # Some curl transports report an HTTP rejection as 56 rather than 22.
        case "$download_exit" in
            22|56)
                release_unavailable=1
                warn "HTTP 404: $1; pretending we can continue..."
                return 0 ;;
        esac
    fi
    cat "$2.error" >&2
    die "download failed: $1; check the release/tag and network connection"
}

prepare_repo_source() {
    # Build a temporary inventory from checkout filenames, without packaging,
    # compiling, signing, or executing any code from the selected repository.
    release=$work/repo-preview
    plugin=$release/plugin
    mkdir -p "$plugin/skills" "$plugin/agents" "$plugin/codex/agents"
    for skill in "$from_repo"/leanck-skills/*; do
        [ -d "$skill" ] || continue
        [ -f "$skill/SKILL.md" ] || die "missing skill metadata: $skill/SKILL.md"
        valid_text "${skill##*/}"
        mkdir "$plugin/skills/${skill##*/}"
    done
    for agent in "$from_repo"/leanck-subagents/*.md; do
        [ -f "$agent" ] || continue
        valid_text "${agent##*/}"
        : > "$plugin/agents/${agent##*/}"
    done
    for agent in "$from_repo"/leanck-subagents/*.codex.toml; do
        [ -f "$agent" ] || continue
        name=${agent##*/}; name=${name%.codex.toml}
        valid_text "$name"
        : > "$plugin/codex/agents/$name.toml"
    done
    source_version=$(awk -F '"' '/^version = "/ {print $2; exit}' "$from_repo/crates/leanck-cli/Cargo.toml")
    case "$source_version" in ''|*[!0-9A-Za-z.+-]*) die 'cannot read the local CLI version from Cargo.toml' ;; esac
    if [ "$dry_run" = 1 ]; then
        installed_version="v$source_version (local source preview; release unverified)"
        plan "Source inventory: $from_repo (offline; no binary built or release verified)"
        say 'Using local source inventory; no network access, build, signing, or host commands.'
    else
        installed_version="v$source_version (local source)"
        plan "Local source: $from_repo"
        say 'Using local source; building and packaging start after approval.'
    fi
}

repo_command() {
    # run_bounded starts this in a child; exec keeps its cancellation PID valid.
    cd "$from_repo" || exit 1
    exec "$@"
}
build_repo_source() {
    run_bounded "$timeout" python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)' ||
        die 'local source installation requires Python 3.11 or newer on PATH'
    if [ "$cli" = 1 ]; then
        for command in cargo rustc; do
            command -v "$command" >/dev/null 2>&1 || die "local builds require $command on PATH"
        done
        run_bounded "$timeout" repo_command rustc -vV > "$work/rustc-version" || die 'could not determine the native Rust target'
        native_target=$(awk '/^host: / {print $2; exit}' "$work/rustc-version")
        case "$native_target" in ''|*[!0-9A-Za-z_-]*) die 'could not determine the native Rust target' ;; esac
        say "Building leanck from $from_repo"
        run_bounded "$timeout" repo_command cargo build --release --locked --package leanck-cli \
            --manifest-path "$from_repo/Cargo.toml" --target-dir "$from_repo/target" --target "$native_target" >&2 ||
            die 'local build failed; check the build output, or increase --timeout for a slow build'
        local_bin_dir=$from_repo/target/$native_target/release
    fi
    say 'Packaging local skills, agents, and plugin configuration'
    if [ "$cli" = 1 ]; then
        run_bounded "$timeout" python3 "$from_repo/packaging/package_local.py" \
            --bin-dir "$local_bin_dir" --output "$work/local-install" --version "$source_version" --inventory "$release" >&2 ||
            die 'could not package local source; no installation files were changed'
    else
        run_bounded "$timeout" python3 "$from_repo/packaging/package_local.py" \
            --output "$work/local-install" --version "$source_version" --inventory "$release" >&2 ||
            die 'could not package local source; no installation files were changed'
    fi
    release=$work/local-install
    plugin=$release/plugin
    plugin_release=$release
}

prepare_preview() {
    # Temporary inventory only: no downloaded files are trusted or executed.
    # Keep these expected component names in sync with leanck-skills/ and leanck-subagents/.
    release=$work/preview
    plugin=$release/plugin
    for skill in lean-proving lean-simplify lean-style; do
        mkdir -p "$plugin/skills/$skill"
    done
    mkdir -p "$plugin/agents" "$plugin/codex/agents"
    for agent in lean-expert lean-grind lean-style-worker; do
        : > "$plugin/agents/$agent.md"
        : > "$plugin/codex/agents/$agent.toml"
    done
    if [ "$version" = latest ]; then
        installed_version='latest (unresolved; unverified)'
    else
        installed_version="v$version (unverified)"
    fi
    warn 'Could not verify contents, checksum, or version for dry-run.'
}

stop_progress() {
    if [ -n "${progress_pid:-}" ]; then
        kill "$progress_pid" 2>/dev/null || :
        wait "$progress_pid" 2>/dev/null || :
        progress_pid=''
        printf '\r                        \r' >&2
    fi
}

check_destination() {
    if [ -L "$1" ] || [ -e "$1" ]; then
        if [ -L "$1" ] || [ ! -f "$1/.leanck-installed" ] ||
            [ "$(cat "$1/.leanck-installed")" != 'leanck installer v1' ]; then
            conflict "$1"
        elif [ ! -f "$1/.leanck-content-sha256" ] ||
            [ "$(cat "$1/.leanck-content-sha256")" != "$(tree_digest "$1")" ] ||
            [ -n "$(find "$1" -type l -print)" ]; then
            conflict "$1"
        fi
    fi
}

install_tree() {
    source_tree=$1
    destination=$2
    skipped "$destination" && return 0
    if preview; then
        plan "Would install $destination"
        return 0
    fi
    mkdir -p "$(dirname "$destination")"
    pending=$(mktemp -d "$(dirname "$destination")/.leanck-install.XXXXXX")
    cp -R "$source_tree/." "$pending/"
    printf '%s\n' 'leanck installer v1' > "$pending/.leanck-installed"
    tree_digest "$pending" > "$pending/.leanck-content-sha256"
    commit "$pending" "$destination"
    if [ "$source_tree" = "$plugin_release" ]; then component_source=.
    else component_source=plugin/${source_tree#"$plugin/"}; fi
    record_install tree "$(cat "$destination/.leanck-content-sha256")" "$destination" "$component_source"
    say "Installed $destination"
}

install_agents() {
    agent_source=$1
    agent_destination=$2
    agent_pattern=$3
    agent_links=${4:-}
    if preview; then
        for agent in "$agent_source"/$agent_pattern; do
            skipped "$agent_destination/${agent##*/}" || plan "Would install $agent_destination/${agent##*/}"
        done
        return 0
    fi
    mkdir -p "$agent_destination"
    for agent in "$agent_source"/$agent_pattern; do
        skipped "$agent_destination/${agent##*/}" && continue
        # Each agent gets sidecars; never claim ownership of the host's directory.
        pending=$(mktemp "$agent_destination/.leanck-install.XXXXXX")
        if [ -n "$agent_links" ]; then
            rm "$pending"
            ln -s "$agent_links/${agent##*/}" "$pending"
        else
            cp "$agent" "$pending"
            chmod 644 "$pending"
        fi
        commit "$pending" "$agent_destination/${agent##*/}"
        for suffix in leanck-installed leanck-sha256; do
            pending=$(mktemp "$agent_destination/.leanck-install.XXXXXX")
            if [ "$suffix" = leanck-installed ]; then printf '%s\n' 'leanck installer v1' > "$pending"
            else digest_file "$agent_destination/${agent##*/}" > "$pending"; fi
            commit "$pending" "$agent_destination/${agent##*/}.$suffix"
        done
        if [ -n "$agent_links" ]; then
            record_install link "$agent_links/${agent##*/}" "$agent_destination/${agent##*/}" "$agent_links/${agent##*/}"
        else
            record_install agent "$(digest_file "$agent_destination/${agent##*/}")" "$agent_destination/${agent##*/}" "plugin/${agent#"$plugin/"}"
        fi
    done
    say "Installed agents in $agent_destination"
}

check_agents() {
    for agent in "$1"/$3; do
        agent_dest=$2/${agent##*/}
        [ ! -d "$agent_dest" ] || die "$agent_dest is a directory"
        if [ -e "$agent_dest" ] || [ -L "$agent_dest" ]; then
            if { [ -L "$agent_dest" ] && { [ -z "${4:-}" ] || [ "$(readlink "$agent_dest")" != "$4/${agent##*/}" ]; }; } ||
                [ ! -f "$agent_dest.leanck-installed" ] ||
                [ "$(cat "$agent_dest.leanck-installed")" != 'leanck installer v1' ]; then
                conflict "$agent_dest"
            elif [ ! -f "$agent_dest" ] || [ ! -f "$agent_dest.leanck-sha256" ] ||
                [ "$(cat "$agent_dest.leanck-sha256")" != "$(digest_file "$agent_dest")" ]; then
                conflict "$agent_dest"
            fi
        fi
        [ ! -L "$agent_dest.leanck-installed" ] || die "refusing symlink: $agent_dest.leanck-installed"
        [ ! -e "$agent_dest.leanck-installed" ] || [ -f "$agent_dest.leanck-installed" ] ||
            die "invalid ownership marker: $agent_dest.leanck-installed"
    done
}

unchanged_managed_agent() {
    [ -f "$1" ] && [ -f "$1.leanck-installed" ] && [ ! -L "$1.leanck-installed" ] &&
        [ -f "$1.leanck-sha256" ] && [ ! -L "$1.leanck-sha256" ] &&
        [ "$(cat "$1.leanck-installed")" = 'leanck installer v1' ] &&
        [ "$(cat "$1.leanck-sha256")" = "$(digest_file "$1")" ]
}
plan_codex_agent_migration() {
    : > "$work/codex-agent-migration"
    if [ "$agents_codex" != 1 ]; then
        [ "$mode" = plugin ] && [ "$plugin_codex" = 1 ] && ! skipped "$bundle_dir" || return 0
    fi
    for agent in "$plugin"/codex/agents/*.toml; do
        [ -f "$agent" ] || continue
        legacy_agent=$codex_dir/agents/${agent##*/}
        skipped "$codex_agent_path/${agent##*/}" && continue
        if unchanged_managed_agent "$legacy_agent"; then
            printf '%s\n' "$legacy_agent" >> "$work/codex-agent-migration"
            plan "Move managed Codex agent $legacy_agent into $codex_agent_path/"
        fi
    done
}
migrate_codex_agents() {
    while IFS= read -r legacy_agent; do
        unchanged_managed_agent "$legacy_agent" || die "$legacy_agent changed after review; rerun the installer"
        for suffix in '' .leanck-installed .leanck-sha256; do commit - "$legacy_agent$suffix"; done
    done < "$work/codex-agent-migration"
}

cleanup() {
    exit_status=$?
    trap - 0
    set +e
    printf '%s' "$reset" >&2
    if [ "${form_active:-0}" = 1 ]; then printf '\033[?2004l\033[?7h' >&2; fi
    if [ "$interactive" = 1 ] && [ "${TERM:-}" != dumb ]; then printf '\033[?25h' >&2; fi
    stop_command
    stop_progress
    if [ "$exit_status" != 0 ]; then
        outcome=${outcome:-failed}
        if [ -n "$work" ] && [ "$committed" = 0 ]; then rollback; fi
        if [ "$host_started" = 1 ]; then warn 'Host plugin registration may be partial. Rerun the same installer selections to finish registration.'; fi
        [ -n "$error" ] || error="Installer exited with status $exit_status; rerun with --verbose for details"
    fi
    emit_json
    if [ -n "$pending" ]; then run_bounded 10 rm -rf "$pending"; fi
    if [ -n "$work" ]; then run_bounded 10 rm -rf "$work"; fi
    exit "$exit_status"
}
load_state() {
    [ -f "$state_dir/selections.tsv" ] || return 0
    tab=$(printf '\t')
    while IFS="$tab" read -r key value; do
        valid_text "$value"
        case "$key" in
            extra_skills|extra_claude_agents|extra_codex_agents) append_location "$key" "$value" ;;
            cli|skills_claude|skills_codex|agents_claude|agents_codex|add_to_path)
                case "$value" in 0|1) eval "$key=\$value" ;; esac ;;
            bin_dir) standalone_bin_dir=$value ;;
            version|data_dir|claude_dir|codex_dir|codex_skills|codex_agents_dir|completions_dir|fish_completions_dir|path_file|completions|shell|last_binary_path|last_binary_digest|last_completions_path|last_completions_digest|standalone_bin_dir|plugin_bin_dir)
                eval "$key=\$value" ;;
        esac
    done < "$state_dir/selections.tsv"
}
save_state() {
    mkdir -p "$state_dir"
    pending=$(mktemp "$state_dir/.leanck-install.XXXXXX")
    if [ -f "$state_dir/uninstall.tsv" ]; then cat "$state_dir/uninstall.tsv" >> "$pending"; fi
    if [ -f "$work/uninstall.tsv" ]; then cat "$work/uninstall.tsv" >> "$pending"; fi
    if [ "$mode" = plugin ] && ! skipped "$bundle_dir"; then
        [ "$plugin_claude" = 0 ] || printf 'host\tclaude\t%s\n' "$claude_dir" >> "$pending"
        [ "$plugin_codex" = 0 ] || printf 'host\tcodex\t%s\n' "$codex_dir" >> "$pending"
    fi
    # Last installation wins for a path; host records are scoped to the host home.
    awk -F '\t' '{key=($1=="host" ? $1 FS $2 FS $3 : $3); lines[key]=$0} END {for(key in lines) print lines[key]}' "$pending" > "$work/inventory"
    cat "$work/inventory" > "$pending"
    commit "$pending" "$state_dir/uninstall.tsv"
    if [ "$cli" = 1 ] && ! skipped "$bin_dir/leanck"; then
        last_binary_path=$bin_dir/leanck; last_binary_digest=$(digest_file "$bin_dir/leanck")
    fi
    completion_destination
    if [ "$cli" = 1 ] && [ "$completions" != none ] && ! skipped "$completion_path"; then
        last_completions_path=$completion_path
        last_completions_digest=$(digest_file "$last_completions_path")
    fi
    pending=$(mktemp "$state_dir/.leanck-install.XXXXXX")
    if [ "$mode" = standalone ]; then standalone_bin_dir=$bin_dir; fi
    if [ "$mode" = plugin ]; then
        plugin_bin_dir=''; [ "$bin_dir" = "$data_dir/bin" ] || plugin_bin_dir=$bin_dir
    fi
    installed_target=$arch-$platform
    for key in mode plugin_claude plugin_codex from_repo release_url installed_version installed_target version last_binary_path last_binary_digest last_completions_path last_completions_digest cli skills_claude skills_codex agents_claude agents_codex add_to_path standalone_bin_dir plugin_bin_dir data_dir claude_dir codex_dir codex_skills codex_agents_dir completions_dir fish_completions_dir path_file completions shell; do
        eval 'value=$'"$key"
        printf '%s\t%s\n' "$key" "$value" >> "$pending"
    done
    write_location_values >> "$pending"
    commit "$pending" "$state_dir/selections.tsv"
    if [ "$cli" = 1 ] && ! skipped "$bin_dir/leanck" && [ -f "$release/.leanck-update-protocol" ]; then
        pending=$(mktemp "$state_dir/.leanck-install.XXXXXX")
        cat "$work/sources.tsv" > "$pending"
        commit "$pending" "$state_dir/sources.tsv"
        pending=$(mktemp "$state_dir/.leanck-install.XXXXXX")
        run_bounded "$timeout" "$bin_dir/leanck" _update receipt --state-dir "$state_dir" --binary "$bin_dir/leanck" > "$pending" || die 'could not record native update ownership'
        commit "$pending" "$state_dir/receipt.json"
        pending=$(mktemp "$bin_dir/.leanck-install.XXXXXX")
        { printf '{"state_dir":'; json_string "$state_dir"; printf '}\n'; } > "$pending"
        commit "$pending" "$bin_dir/.leanck-installation.json"
        printf 'file\t%s\t%s\n' "$(digest_file "$bin_dir/.leanck-installation.json")" "$bin_dir/.leanck-installation.json" >> "$state_dir/uninstall.tsv"
        if [ "$mode" = plugin ] && ! skipped "$bundle_dir"; then
            pending=$(mktemp "$bundle_dir/plugin/bin/.leanck-install.XXXXXX")
            { printf '{"state_dir":'; json_string "$state_dir"; printf '}\n'; } > "$pending"
            commit "$pending" "$bundle_dir/plugin/bin/.leanck-installation.json"
            # The discovery marker is installation metadata; exclude it from tree content hashes.
            tree_digest "$bundle_dir" > "$bundle_dir/.leanck-content-sha256"
        fi
    fi
}
show_plan() {
    printf '\n%sReady to install leanck %s%s\n' "$bold" "$installed_version" "$reset" >&2
    plan_lines=$(wc -l < "$work/plan")
    if [ "$no_pager" = 0 ] && [ "$json" = 0 ] && [ -t 2 ] && [ -t 1 ] &&
        [ "$plan_lines" -gt 20 ] && command -v less >/dev/null 2>&1; then
        less -FRX "$work/plan" >&2 || :
    else
        cat "$work/plan" >&2
    fi
    printf '\n' >&2
}
display_path() {
    case "$1" in "$HOME") printf '~' ;; "$HOME"/*) printf '~/%s' "${1#"$HOME"/}" ;; *) printf '%s' "$1" ;; esac
}
inspect_config() {
    printf '\n`leanck` installation config:\n' >&2
    if [ "$mode" = plugin ]; then
        printf '%s%s\n' '- Plugin data dir: ' "$(display_path "$data_dir")" >&2
        printf '%s\n' '- Install skills: [bundled with plugin]' '- Claude agents: [bundled with plugin]' '- MCP: enabled' >&2
        host_default "$plugin_claude" "$plugin_codex"; printf '%s%s\n' '- Register plugins: ' "$host_choice" >&2
        if [ "$plugin_codex" = 1 ]; then
            printf '%s%s\n' '- Codex custom agents: ' "$(display_path "${codex_agents_dir:-$codex_dir/agents/leanck}")" >&2
        fi
    else
        host_default "$skills_claude" "$skills_codex"; printf '%s%s\n' '- Skills: ' "$host_choice" >&2
        host_default "$agents_claude" "$agents_codex"; printf '%s%s\n' '- Agents: ' "$host_choice" >&2
        printf '%s\n' '- MCP: disabled (available through leanck serve mcp)' >&2
        if [ "$agents_codex" = 1 ]; then
            printf '%s%s\n' '- Codex custom agents: ' "$(display_path "${codex_agents_dir:-$codex_dir/agents/leanck}")" >&2
        fi
    fi
    if [ "$cli" = 1 ]; then printf '%s%s\n' '- Executable: ' "$(display_path "$bin_dir/leanck")" >&2; fi
    write_location_values | while IFS="$(printf '\t')" read -r key location; do
        case "$key" in extra_skills) label=skills ;; extra_claude_agents) label='Claude agents' ;; extra_codex_agents) label='Codex agents' ;; esac
        printf '%s%s: %s\n' '- Additional ' "$label" "$(display_path "$location")" >&2
    done
    printf '%s%s\n' '- Shell completions: ' "$completions" >&2
    if [ "$completions" != none ]; then
        completion_destination
        printf '%s%s\n' '- Completions directory: ' "$(display_path "$completion_dir")" >&2
    fi
    if [ "$add_to_path" = 1 ]; then
        printf '%s\n' '- Add leanck to PATH: yes' >&2
        printf '%s%s\n' '- Shell: ' "$shell" >&2
        printf '%s%s\n' '- Shell profile: ' "$(display_path "$path_file")" >&2
    else printf '%s\n' '- Add leanck to PATH: no' >&2; fi
    printf '\n' >&2
}
edit_advanced_config_plain() {
    plain_initial_config=$(config_snapshot)
    while :; do
        inspect_config
        if [ "$mode" = plugin ]; then
            choose 'Edit advanced config' data 'data binary skills agents completions path hosts save cancel' \
                'Plugin data directory|Executable directory|Install skills|Install agents|Shell completions|Add leanck to PATH|Register plugins|Save changes|Cancel'
        else
            choose 'Edit advanced config' binary 'binary skills agents completions path save cancel' \
                'Executable|Install skills|Install agents|Shell completions|Add leanck to PATH|Save changes|Cancel'
        fi
        case "$reply" in
            data)
                previous_data_dir=$data_dir
                prompt 'Plugin data directory' "$data_dir" text; absolute_path "$reply"; data_dir=$resolved
                if [ "$bin_dir" = "$previous_data_dir/bin" ]; then bin_dir=$data_dir/bin; fi ;;
            binary)
                if [ "$mode" = standalone ]; then
                    yes_no 'Install the CLI?' "$( [ "$cli" = 1 ] && printf yes || printf no )"; cli=$answer
                fi
                if [ "$cli" = 1 ]; then prompt 'Executable directory' "$bin_dir" text; bin_dir=$reply; fi ;;
            hosts)
                host_default "$plugin_claude" "$plugin_codex"; choose_hosts plugin 'Register plugins' "$host_choice"
                if [ "$plugin_claude" = 1 ]; then prompt 'Claude configuration directory' "$claude_dir" text; claude_dir=$reply; fi
                if [ "$plugin_codex" = 1 ]; then prompt 'Codex configuration directory' "$codex_dir" text; codex_dir=$reply; fi ;;
            skills)
                if [ "$mode" = standalone ]; then
                    host_default "$skills_claude" "$skills_codex"; choose_hosts skills 'Install skills' "$host_choice"
                    if [ "$skills_claude" = 1 ]; then prompt 'Claude configuration directory' "$claude_dir" text; claude_dir=$reply; fi
                    if [ "$skills_codex" = 1 ]; then prompt 'Codex skills directory' "$codex_skills" text; codex_skills=$reply; fi
                fi
                prompt 'Additional skills directory (keep / none / path)' keep text
                case "$reply" in keep) ;; none) extra_skills='' ;; *) append_location extra_skills "$reply" ;; esac ;;
            agents)
                if [ "$mode" = standalone ]; then
                    host_default "$agents_claude" "$agents_codex"; choose_hosts agents 'Install agents' "$host_choice"
                    if [ "$agents_claude" = 1 ]; then prompt 'Claude configuration directory' "$claude_dir" text; claude_dir=$reply; fi
                fi
                if [ "$agents_codex" = 1 ] || [ "$plugin_codex" = 1 ]; then
                    codex_agents_destination
                    prompt 'Codex agents directory' "$codex_agent_path" text; codex_agents_dir=$reply
                fi
                prompt 'Additional Claude agents directory (keep / none / path)' keep text
                case "$reply" in keep) ;; none) extra_claude_agents='' ;; *) append_location extra_claude_agents "$reply" ;; esac
                prompt 'Additional Codex agents directory (keep / none / path)' keep text
                case "$reply" in keep) ;; none) extra_codex_agents='' ;; *) append_location extra_codex_agents "$reply" ;; esac ;;
            completions) configure_completions advanced ;;
            path)
                yes_no 'Add leanck to PATH?' "$( [ "$add_to_path" = 1 ] && printf yes || printf no )"
                add_to_path=$answer
                if [ "$add_to_path" = 1 ]; then
                    while :; do
                        prompt 'Shell (fish / bash / zsh)' "$shell" choice
                        case "$reply" in fish|bash|zsh) break ;; esac
                        warn 'Choose fish, bash, or zsh.'
                    done
                    if [ "$shell" != "$reply" ]; then
                        shell=$reply
                        case "$shell" in
                            fish) path_file=${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish ;;
                            bash) path_file=$HOME/.bashrc ;;
                            zsh) path_file=$HOME/.zshrc ;;
                        esac
                    fi
                    prompt 'Shell profile to update' "$path_file" text; path_file=$reply
                fi ;;
            save) return ;;
            cancel)
                if [ "$plain_initial_config" != "$(config_snapshot)" ]; then
                    yes_no 'Discard advanced config changes?' no
                    [ "$answer" = 1 ] || continue
                fi
                return 2 ;;
        esac
    done
}
write_advanced_config_values() {
    for key in cli bin_dir data_dir plugin_hosts skills_hosts agents_hosts claude_dir codex_dir codex_skills \
        codex_agents_dir completions completions_dir fish_completions_dir add_to_path shell path_file; do
        case "$key" in
            plugin_hosts) host_default "$plugin_claude" "$plugin_codex"; value=$host_choice ;;
            skills_hosts) host_default "$skills_claude" "$skills_codex"; value=$host_choice ;;
            agents_hosts) host_default "$agents_claude" "$agents_codex"; value=$host_choice ;;
            cli|add_to_path) eval 'value=$'"$key"; if [ "$value" = 1 ]; then value=yes; else value=no; fi ;;
            *) eval 'value=$'"$key" ;;
        esac
        printf '%s\t%s\n' "$key" "$value"
    done
    write_location_values
}
load_advanced_config_values() {
    extra_skills='' extra_claude_agents='' extra_codex_agents=''
    tab=$(printf '\t')
    while IFS="$tab" read -r key value; do
        valid_text "$value"
        case "$key" in
            extra_skills|extra_claude_agents|extra_codex_agents) append_location "$key" "$value" ;;
            plugin_hosts|skills_hosts|agents_hosts)
                group=${key%_hosts}; eval "${group}_claude=0; ${group}_codex=0"; select_hosts "$group" "$value" ;;
            cli|add_to_path) if [ "$value" = yes ]; then eval "$key=1"; else eval "$key=0"; fi ;;
            bin_dir|data_dir|claude_dir|codex_dir|codex_skills|codex_agents_dir|completions|completions_dir|fish_completions_dir|shell|path_file)
                eval "$key=\$value" ;;
        esac
    done < "$1"
}
edit_advanced_config() {
    form_values=$work/advanced-config.values
    if [ "${TERM:-}" = dumb ]; then
        if (edit_advanced_config_plain && write_advanced_config_values > "$form_values"); then
            load_advanced_config_values "$form_values"
        else
            form_status=$?
            [ "$form_status" = 2 ] || cancel
        fi
        return 0
    fi
    write_advanced_config_values > "$form_values"
    # Keep the form self-contained so curl | sh needs no additional assets or runtime.
    cat > "$work/advanced-config.bash" <<'LEANCK_CONFIG_FORM'
set -u
values_file=$1 mode=$2 prompt_style=$3 selected_style=$4 error_style=$5 reset_style=$6
all_fields=(cli bin_dir data_dir plugin_hosts skills_hosts agents_hosts claude_dir codex_dir codex_skills
    codex_agents_dir completions completions_dir fish_completions_dir add_to_path shell path_file)
extra_skills_count=0 extra_claude_agents_count=0 extra_codex_agents_count=0 pending_field=''
while IFS=$'\t' read -r field value; do
    case "$field" in
        extra_skills|extra_claude_agents|extra_codex_agents)
            counter=${field}_count; count=${!counter}
            printf -v "${field}_$count" '%s' "$value"
            printf -v "$counter" '%s' "$((count + 1))" ;;
        cli|bin_dir|data_dir|plugin_hosts|skills_hosts|agents_hosts|claude_dir|codex_dir|codex_skills|codex_agents_dir|completions|completions_dir|fish_completions_dir|add_to_path|shell|path_file)
            printf -v "$field" '%s' "$value" ;;
    esac
done < "$values_file"
write_form_values() {
    local field group counter count i
    for field in "${all_fields[@]}"; do printf '%s\t%s\n' "$field" "${!field}"; done
    for group in extra_skills extra_claude_agents extra_codex_agents; do
        counter=${group}_count; count=${!counter}
        for ((i=0; i<count; i++)); do
            field=${group}_$i
            [[ -z ${!field} ]] || printf '%s\t%s\n' "$group" "${!field}"
        done
    done
}
initial_config=$(write_form_values)
add_location_fields() {
    local group=$1 counter count i field
    counter=${group}_count; count=${!counter}
    for ((i=0; i<count; i++)); do
        field=${group}_$i
        if [[ -n ${!field} || $field == "$pending_field" ]]; then
            fields+=("$field"); labels+=("  $2")
        fi
    done
}
build_fields() {
    if [[ $mode == plugin ]]; then
        fields=(data_dir bin_dir skills_bundle)
        labels=('Plugin data directory' 'Executable directory' 'Install skills')
        context='Plugin installation; MCP enabled'
    else
        fields=(cli bin_dir skills_hosts)
        labels=('Install executable' 'Executable directory' 'Install skills')
        context='Standalone installation'
        if [[ $skills_hosts == claude || $skills_hosts == both ]]; then fields+=(claude_dir); labels+=('  Claude configuration'); fi
        if [[ $skills_hosts == codex || $skills_hosts == both ]]; then fields+=(codex_skills); labels+=('  Codex skills directory'); fi
    fi
    add_location_fields extra_skills 'Additional location'
    fields+=(add_extra_skills); labels+=('  Add skill location...')
    if [[ $mode == plugin ]]; then fields+=(agents_bundle); else fields+=(agents_hosts); fi
    labels+=('Install agents')
    if [[ $mode == standalone && ( $agents_hosts == claude || $agents_hosts == both ) && $skills_hosts != claude && $skills_hosts != both ]]; then
        fields+=(claude_dir); labels+=('  Claude configuration')
    fi
    if [[ ( $mode == plugin && ( $plugin_hosts == codex || $plugin_hosts == both ) ) || ( $mode == standalone && ( $agents_hosts == codex || $agents_hosts == both ) ) ]]; then
        fields+=(codex_agents_dir); labels+=('  Codex agents directory')
    fi
    add_location_fields extra_claude_agents 'Claude location'
    add_location_fields extra_codex_agents 'Codex location'
    fields+=(add_extra_claude_agents add_extra_codex_agents completions)
    labels+=('  Add Claude location...' '  Add Codex location...' 'Shell completions')
    if [[ $completions != none ]]; then fields+=(completions_dir); labels+=('  Completions directory'); fi
    fields+=(add_to_path); labels+=('Add leanck to PATH')
    if [[ $add_to_path == yes ]]; then fields+=(shell path_file); labels+=('  Shell' '  Shell profile'); fi
    if [[ $mode == plugin ]]; then
        fields+=(plugin_hosts); labels+=('Register plugins')
        if [[ $plugin_hosts == claude || $plugin_hosts == both ]]; then fields+=(claude_dir); labels+=('  Claude configuration'); fi
        if [[ $plugin_hosts == codex || $plugin_hosts == both ]]; then fields+=(codex_dir); labels+=('  Codex configuration'); fi
    fi
    fields+=(save cancel); labels+=('Save changes' 'Cancel')
}
build_fields
saved=$(stty -g <&3) || exit 1
trap 'printf "%s\033[?2004l\033[?7h\033[?25h" "$reset_style" >&2; stty "$saved" <&3' EXIT
trap 'exit 130' INT TERM HUP
stty -echo -icanon -isig min 1 time 0 <&3 || exit 1
printf '\033[?2004h\033[?7l\033[?25l' >&2
index=0 top=0 editing=0 rendered=0 block_height=0 current_row=0
buffer='' cursor=0 offset=0 message='' pasting=0 paste_invalid=0
confirm_cancel=0 discard_index=0

anchor() {
    printf '\r' >&2
    if ((current_row < block_height)); then printf '\033[%sB' "$((block_height - current_row))" >&2; fi
    current_row=$block_height
}
display_value() {
    case "$1" in
        *_dir|path_file|codex_skills|extra_*)
            case "$2" in "$HOME") printf '~' ;; "$HOME"/*) printf '~/%s' "${2#"$HOME"/}" ;; *) printf '%s' "$2" ;; esac ;;
        *) printf '%s' "$2" ;;
    esac
}
completion_default_dir() {
    case "$1" in
        fish) printf '%s' "$fish_completions_dir" ;;
        bash) printf '%s/bash-completion/completions' "${XDG_DATA_HOME:-$HOME/.local/share}" ;;
        zsh) printf '%s/zsh/site-functions' "${XDG_DATA_HOME:-$HOME/.local/share}" ;;
        powershell) printf '%s/powershell/Completions' "${XDG_CONFIG_HOME:-$HOME/.config}" ;;
        elvish) printf '%s/elvish/lib' "${XDG_CONFIG_HOME:-$HOME/.config}" ;;
    esac
}
field_value() {
    local field=$1
    case "$field" in
        skills_bundle) printf '[bundled with plugin]' ;;
        agents_bundle)
            case "$plugin_hosts" in
                claude|both) printf 'Claude [bundled with plugin]' ;;
                codex) printf 'Codex' ;;
                *) printf '[bundled with plugin]' ;;
            esac ;;
        codex_agents_dir) printf '%s' "${codex_agents_dir:-$codex_dir/agents/leanck}" ;;
        completions_dir) printf '%s' "${completions_dir:-$(completion_default_dir "$completions")}" ;;
        *) printf '%s' "${!field}" ;;
    esac
}
render_form() {
    local rows columns count visible old_height label_width available i marker style label shown prefix row_value
    printf '\033[?25l' >&2
    read -r rows columns < <(stty size <&3)
    ((rows > 4)) || rows=24
    ((columns > 12)) || columns=80
    build_fields
    count=${#fields[@]}; ((index < count)) || index=$((count - 1))
    visible=$((rows - 4)); ((visible <= count)) || visible=$count
    ((index >= top)) || top=$index
    ((index < top + visible)) || top=$((index - visible + 1))
    ((top + visible <= count)) || top=$((count - visible))
    label_width=$((columns / 2)); ((label_width <= 30)) || label_width=30
    available=$((columns - label_width - 5)); ((available > 0)) || available=1
    old_height=$block_height
    if ((rendered)); then
        anchor
        printf '\033[%sA' "$block_height" >&2
    else printf '\n' >&2; fi
    if ((confirm_cancel)); then
        printf '\r\033[2K%sDiscard advanced config changes?%s\n' "$prompt_style" "$reset_style" >&2
        printf '\r\033[2KYour changes have not been saved.\n' >&2
        for ((i=0; i<2; i++)); do
            marker='  ' style=$reset_style
            if ((i == discard_index)); then marker='> '; style=$selected_style; fi
            if ((i == 0)); then label='Keep editing'; else label='Discard changes'; fi
            printf '\r\033[2K%s%s%s%s\n' "$style" "$marker" "$label" "$reset_style" >&2
        done
        block_height=4; current_row=$block_height; rendered=1
        printf '\r\033[2K\033[J' >&2
        return
    fi
    printf '\r\033[2K%sAdvanced configuration%s\n' "$prompt_style" "$reset_style" >&2
    printf '\r\033[2K%s\n' "${context:0:$((columns - 1))}" >&2
    for ((i=top; i<top+visible; i++)); do
        marker='  ' style=$reset_style
        if ((i == index)); then marker='> '; style=$selected_style; fi
        if [[ ${fields[i]} == save || ${fields[i]} == cancel || ${fields[i]} == add_extra_* ]]; then
            printf '\r\033[2K%s%s%s%s\n' "$style" "$marker" "${labels[i]}" "$reset_style" >&2
            continue
        fi
        printf -v label '%-*s' "$label_width" "${labels[i]:0:label_width}"
        prefix="$marker$label: "
        if ((editing && i == index)); then
            ((cursor >= offset)) || offset=$cursor
            ((cursor <= offset + available)) || offset=$((cursor - available))
            row_value=${buffer:offset:available}
            printf '\r\033[2K%s%s%s%s\n' "$prompt_style" "$prefix" "$reset_style" "$row_value" >&2
        else
            field=${fields[i]}; shown=$(display_value "$field" "$(field_value "$field")")
            printf '\r\033[2K%s%s%s%s\n' "$style" "$prefix" "${shown:0:available}" "$reset_style" >&2
        fi
    done
    block_height=$((visible + 2)); current_row=$block_height; rendered=1
    printf '\r\033[2K%s%s%s' "$error_style" "${message:0:$((columns - 1))}" "$reset_style" >&2
    ((old_height <= block_height)) || printf '\033[J' >&2
    if ((editing)); then
        current_row=$((index - top + 2))
        printf '\r\033[%sA' "$((block_height - current_row))" >&2
        printf -v label '%-*s' "$label_width" "${labels[index]:0:label_width}"
        printf '%s> %s: %s%s\033[?25h' "$prompt_style" "$label" "$reset_style" "${buffer:offset:cursor-offset}" >&2
    else printf '\033[?25l' >&2; fi
}
read_key() {
    local raw sequence byte
    IFS= read -r -s -n 1 raw <&3 || exit 130
    char='' key=ignore
    case "$raw" in
        '') key=enter ;;
        $'\003'|$'\004') exit 130 ;;
        $'\177'|$'\010') key=backspace ;;
        $'\001') key=home ;;
        $'\005') key=end ;;
        $'\025') key=clear ;;
        $'\013') key=kill ;;
        $'\033')
            # A short terminal read distinguishes Escape from an arrow sequence on Bash 3.2.
            stty min 0 time 1 <&3
            sequence=$(dd bs=1 count=1 <&3 2>/dev/null)
            if [[ $sequence == '[' || $sequence == O ]]; then
                while ((${#sequence} < 12)); do
                    byte=$(dd bs=1 count=1 <&3 2>/dev/null)
                    [[ -n $byte ]] || break
                    sequence+=$byte
                    case "$byte" in [A-Za-z~]) break ;; esac
                done
                case "$sequence" in
                    '[A'|OA) key=up ;; '[B'|OB) key=down ;;
                    '[C'|OC) key=right ;; '[D'|OD) key=left ;;
                    '[H'|OH|'[1~'|'[7~') key=home ;; '[F'|OF|'[4~'|'[8~') key=end ;;
                    '[3~') key=delete ;; '[200~') key=paste_start ;; '[201~') key=paste_end ;;
                esac
            else key=escape; fi
            stty min 1 time 0 <&3 ;;
        [[:cntrl:]]) ;;
        *) key=char; char=$raw ;;
    esac
}
default_profile() {
    case "$1" in
        fish) printf '%s/fish/config.fish' "${XDG_CONFIG_HOME:-$HOME/.config}" ;;
        zsh) printf '%s/.zshrc' "$HOME" ;;
        *) printf '%s/.bashrc' "$HOME" ;;
    esac
}
save_field() {
    local field previous candidate
    field=${fields[index]}; candidate=$buffer; previous=${!field}
    case "$field" in
        cli|add_to_path)
            case "$candidate" in yes|y|1) candidate=yes ;; no|n|0) candidate=no ;; *) message='Enter yes or no.'; return 1 ;; esac ;;
        plugin_hosts|skills_hosts|agents_hosts)
            case "$candidate" in none|claude|codex|both) ;; *) message='Enter none, claude, codex, or both.'; return 1 ;; esac ;;
        completions)
            case "$candidate" in none|bash|zsh|fish|powershell|elvish) ;; *) message='Enter none, bash, zsh, fish, powershell, or elvish.'; return 1 ;; esac ;;
        shell)
            case "$candidate" in fish|bash|zsh) ;; *) message='Enter fish, bash, or zsh.'; return 1 ;; esac ;;
        extra_*)
            # Clearing an additional location removes it from the draft.
            if [[ -n $candidate ]]; then
                case "$candidate" in '~') candidate=$HOME ;; '~/'*) candidate=$HOME/${candidate#??} ;;
                    /*) ;; *) candidate=$PWD/$candidate ;; esac
            fi ;;
        completions_dir|codex_agents_dir)
            if [[ -n $candidate ]]; then
                case "$candidate" in '~') candidate=$HOME ;; '~/'*) candidate=$HOME/${candidate#??} ;;
                    /*) ;; *) candidate=$PWD/$candidate ;; esac
            fi
            if [[ $field == completions_dir && $candidate == "$(completion_default_dir "$completions")" ]]; then candidate=''; fi
            if [[ $field == codex_agents_dir && $candidate == "$codex_dir/agents/leanck" ]]; then candidate=''; fi ;;
        *_dir|path_file|codex_skills)
            [[ -n $candidate ]] || { message='Enter a directory or file path.'; return 1; }
            case "$candidate" in '~') candidate=$HOME ;; '~/'*) candidate=$HOME/${candidate#??} ;;
                /*) ;; *) candidate=$PWD/$candidate ;; esac ;;
    esac
    printf -v "$field" '%s' "$candidate"
    if [[ $field == data_dir && $bin_dir == "$previous/bin" ]]; then bin_dir=$data_dir/bin; fi
    if [[ $field == shell && $path_file == "$(default_profile "$previous")" ]]; then path_file=$(default_profile "$shell"); fi
    message=''
    return 0
}
cycle_field() {
    local field=${fields[index]} direction=$1 choices current i next buffer
    case "$field" in
        cli|add_to_path) choices=(no yes) ;;
        plugin_hosts|skills_hosts|agents_hosts) choices=(none claude codex both) ;;
        completions) choices=(none bash zsh fish powershell elvish) ;;
        shell) choices=(bash zsh fish) ;;
        *) return 1 ;;
    esac
    current=-1
    for ((i=0; i<${#choices[@]}; i++)); do
        [[ ${choices[i]} != "${!field}" ]] || current=$i
    done
    if ((current < 0)); then
        if ((direction > 0)); then next=0; else next=$((${#choices[@]} - 1)); fi
    else next=$(((current + direction + ${#choices[@]}) % ${#choices[@]})); fi
    buffer=${choices[next]}
    save_field || return 1
    # Changing hosts can insert fields above this row. Keep the same setting selected.
    build_fields
    for ((i=0; i<${#fields[@]}; i++)); do [[ ${fields[i]} != "$field" ]] || index=$i; done
    return 0
}
is_dirty() {
    [[ $(write_form_values) != "$initial_config" ]]
}
cancel_form() {
    anchor; printf '\n' >&2
    exit 2
}
request_cancel() {
    if is_dirty; then confirm_cancel=1; discard_index=0; message=''
    else cancel_form; fi
}
save_form() {
    local field
    if [[ $mode == standalone && $cli == no && $skills_hosts == none && $agents_hosts == none && $(write_form_values) != *$'\nextra_'* ]]; then
        message='Select an executable, skills, or custom agents before continuing.'; return 1
    fi
    if [[ $add_to_path == yes ]]; then
        case "$shell" in fish|bash|zsh) ;; *) message='Choose fish, bash, or zsh before enabling PATH setup.'; return 1 ;; esac
    fi
    write_form_values > "$values_file"
    anchor; printf '\n' >&2
    exit 0
}
while :; do
    render_form
    read_key
    if ((pasting)); then
        if ((!editing)); then
            [[ $key != paste_end ]] || pasting=0
            continue
        fi
        case "$key" in
            paste_end)
                pasting=0
                if ((paste_invalid)); then buffer=$paste_buffer; cursor=$paste_cursor; message='Paste a single-line value.'; fi ;;
            char) buffer=${buffer:0:cursor}$char${buffer:cursor}; cursor=$((cursor + ${#char})) ;;
            *) paste_invalid=1 ;;
        esac
        continue
    fi
    if ((confirm_cancel)); then
        case "$key" in
            up|down) discard_index=$((1 - discard_index)) ;;
            escape) confirm_cancel=0 ;;
            enter) if ((discard_index == 0)); then confirm_cancel=0; else cancel_form; fi ;;
            paste_start) pasting=1 ;;
        esac
        continue
    fi
    if ((editing)); then
        case "$key" in
            escape) editing=0; pending_field=''; message='' ;;
            enter) if save_field; then editing=0; pending_field=''; fi ;;
            left) ((cursor <= 0)) || cursor=$((cursor - 1)) ;;
            right) ((cursor >= ${#buffer})) || cursor=$((cursor + 1)) ;;
            home) cursor=0 ;; end) cursor=${#buffer} ;;
            clear) buffer=''; cursor=0 ;; kill) buffer=${buffer:0:cursor} ;;
            backspace)
                if ((cursor > 0)); then buffer=${buffer:0:cursor-1}${buffer:cursor}; cursor=$((cursor - 1)); fi ;;
            delete) buffer=${buffer:0:cursor}${buffer:cursor+1} ;;
            char) buffer=${buffer:0:cursor}$char${buffer:cursor}; cursor=$((cursor + ${#char})) ;;
            paste_start) pasting=1; paste_invalid=0; paste_buffer=$buffer; paste_cursor=$cursor ;;
        esac
    else
        case "$key" in
            up) index=$(((index + ${#fields[@]} - 1) % ${#fields[@]})); message='' ;;
            down) index=$(((index + 1) % ${#fields[@]})); message='' ;;
            left) cycle_field -1 || : ;;
            right) cycle_field 1 || : ;;
            escape) request_cancel ;;
            paste_start) pasting=1 ;;
            enter)
                field=${fields[index]}
                case "$field" in
                    save) save_form ;;
                    cancel) request_cancel ;;
                    cli|add_to_path|plugin_hosts|skills_hosts|agents_hosts|completions|shell) cycle_field 1 ;;
                    add_extra_*)
                        group=${field#add_}; counter=${group}_count; count=${!counter}
                        pending_field=${group}_$count
                        printf -v "$pending_field" '%s' ''
                        printf -v "$counter" '%s' "$((count + 1))"
                        build_fields
                        for ((i=0; i<${#fields[@]}; i++)); do [[ ${fields[i]} != "$pending_field" ]] || index=$i; done
                        buffer=''; cursor=0; offset=0; editing=1; message='' ;;
                    skills_bundle|agents_bundle) message='Choose an additional location below to install another copy.' ;;
                    *) buffer=$(display_value "$field" "$(field_value "$field")"); cursor=${#buffer}; offset=0; editing=1; message='' ;;
                esac ;;
        esac
    fi
done
LEANCK_CONFIG_FORM
    form_active=1
    if bash "$work/advanced-config.bash" "$form_values" "$mode" "$prompt_style" "$selected_style" "$red" "$reset"; then
        form_status=0
    else form_status=$?; fi
    form_active=0
    case "$form_status" in
        0) load_advanced_config_values "$form_values" ;;
        2) return 0 ;;
        *) cancel ;;
    esac
}
config_snapshot() {
    for key in cli bin_dir data_dir plugin_claude plugin_codex skills_claude skills_codex agents_claude agents_codex \
        claude_dir codex_dir codex_skills codex_agents_dir completions completions_dir fish_completions_dir add_to_path shell path_file; do
        eval 'value=$'"$key"
        printf '%s\n' "$value"
    done
    write_location_values
}
profile_line() {
    if [ "$shell" = fish ]; then
        quoted=$(printf '%s' "$bin_dir" | sed "s/\\\\/\\\\\\\\/g; s/'/\\\\'/g")
    else
        quoted=$(printf '%s' "$bin_dir" | sed "s/'/'\\\\''/g")
    fi
    case "$shell" in fish) printf "fish_add_path -- '%s'\n" "$quoted" ;;
        *) printf "export PATH='%s':\"\$PATH\"\n" "$quoted" ;; esac
}
apply_extras() {
    completion_destination
    if [ "$cli" = 1 ] && [ "$completions" != none ] && ! skipped "$completion_path"; then
        if preview; then plan "Would install $completions completions: $completion_path"
        else
            pending=$(mktemp "$completion_dir/.leanck-install.XXXXXX")
            # Preserve generator headers: Zsh's compinit reads #compdef on the first line.
            run_bounded "$timeout" "$bin_dir/leanck" _completions "$completions" > "$pending" || die 'completion generation failed'
            chmod 644 "$pending"
            commit "$pending" "$completion_path"
            record_install completion "$(digest_file "$completion_path")" "$completion_path" "$completions"
        fi
    fi
    if [ "$cli" = 1 ] && [ "$add_to_path" = 1 ] && ! skipped "$path_file"; then
        if preview; then
            plan "Would update shell profile: $path_file"
            plan "  $(profile_line)"
        else
            pending=$(mktemp "$(dirname "$path_file")/.leanck-install.XXXXXX")
            cp -p "$path_file" "$pending"
            awk '/^# >>> leanck PATH >>>$/ {skip=1;next} /^# <<< leanck PATH <<<$/ {skip=0;next} !skip' "$path_file" > "$pending"
            printf '\n# >>> leanck PATH >>>\n' >> "$pending"
            profile_line >> "$pending"
            printf '# <<< leanck PATH <<<\n' >> "$pending"
            commit "$pending" "$path_file"
            record_install profile - "$path_file"
        fi
    fi
}
apply_install() {
    if ! preview; then migrate_codex_agents; fi
    if [ "$mode" = plugin ] && ! preview && ! skipped "$bundle_dir"; then
        # Configure a temporary copy only after approval. Python startup may itself
        # write user caches, so even this preparation belongs after confirmation.
        plugin_release=$work/configured
        cp -R "$release" "$plugin_release"
        python3 - "$plugin_release/plugin/.mcp.codex.json" "$bin_dir/leanck" <<'PYCODE'
import json
from pathlib import Path
import sys
path = Path(sys.argv[1])
data = json.loads(path.read_text())
data['mcpServers']['leanck']['command'] = sys.argv[2]
path.write_text(json.dumps(data, indent=2) + '\n')
PYCODE
    fi
    if [ "$cli" = 1 ] && ! skipped "$bin_dir/leanck"; then
        [ ! -d "$bin_dir/leanck" ] || die "$bin_dir/leanck is a directory"
        if preview; then
            plan "Would install $bin_dir/leanck ($installed_version)"
            if [ -e "$bin_dir/leanck" ] || [ -L "$bin_dir/leanck" ]; then plan "  Back up and replace existing executable"; fi
        else
            mkdir -p "$bin_dir"
            pending=$(mktemp "$bin_dir/.leanck-install.XXXXXX")
            cp "$plugin/bin/leanck" "$pending"
            chmod 755 "$pending"
            run_bounded "$timeout" "$pending" --version >&2
            commit "$pending" "$bin_dir/leanck"
            record_install file "$(digest_file "$bin_dir/leanck")" "$bin_dir/leanck" plugin/bin/leanck
            say "Installed $bin_dir/leanck ($installed_version)"
        fi
    fi
    for host in claude codex; do
        if [ "$host" = claude ]; then
            selected=$skills_claude; skills_dir=$claude_dir/skills
        else
            selected=$skills_codex; skills_dir=$codex_skills
        fi
        if [ "$selected" = 1 ]; then
            for skill in "$plugin"/skills/*; do install_tree "$skill" "$skills_dir/${skill##*/}"; done
        fi
    done
    if [ "$agents_claude" = 1 ]; then install_agents "$plugin/agents" "$claude_dir/agents" '*.md'; fi
    if [ "$agents_codex" = 1 ]; then install_agents "$plugin/codex/agents" "$codex_agent_path" '*.toml'; fi
    if [ "$mode" = plugin ] && ! skipped "$bundle_dir"; then
        install_tree "$plugin_release" "$bundle_dir"
        if preview; then
            plan "Pin Codex MCP command to $bin_dir/leanck"
            if [ "$plugin_claude" = 1 ]; then
                plan "Would run: claude plugin marketplace add \"$bundle_dir\""
                plan 'Would run: claude plugin install leanck@leanck-installer --scope user'
            fi
            if [ "$plugin_codex" = 1 ]; then
                plan "Would run: codex plugin marketplace add \"$bundle_dir\""
                plan 'Would run: codex plugin add leanck@leanck-installer'
            fi
        fi
        install_tree "$plugin/codex/agents" "$data_dir/agents"
        if [ "$plugin_codex" = 1 ]; then
            install_agents "$plugin/codex/agents" "$codex_agent_path" '*.toml' "$data_dir/agents"
        fi
    fi
    extra_locations install
    apply_extras
}

register_plugins() {
    [ "$plugin_claude$plugin_codex" != 00 ] || return 0
    skipped "$bundle_dir" && return 0
    # External host configuration cannot be rolled back as a file transaction.
    host_started=1
    # Make the installed CLI available to host registration hooks in this process.
    if [ "$cli" = 1 ]; then PATH=$bin_dir:$PATH; export PATH; fi
    if [ "$plugin_claude" = 1 ]; then
        run_bounded "$timeout" env CLAUDE_CONFIG_DIR="$claude_dir" claude plugin marketplace add "$bundle_dir" >&2 || die 'Claude marketplace registration failed; rerun to resume'
        run_bounded "$timeout" env CLAUDE_CONFIG_DIR="$claude_dir" claude plugin install leanck@leanck-installer --scope user >&2 || die 'Claude plugin installation failed; rerun to resume'
    fi
    if [ "$plugin_codex" = 1 ]; then
        run_bounded "$timeout" env CODEX_HOME="$codex_dir" codex plugin marketplace add "$bundle_dir" >&2 || die 'Codex marketplace registration failed; rerun to resume'
        run_bounded "$timeout" env CODEX_HOME="$codex_dir" codex plugin add leanck@leanck-installer >&2 || die 'Codex plugin installation failed; rerun to resume'
        say 'Codex custom agents are included. Review and trust the Codex hook with /hooks.'
    fi
    say 'Restart your agent host to load the plugin.'
}

prepare_plan() {
    for key in state_dir bin_dir data_dir claude_dir codex_dir codex_skills fish_completions_dir path_file codex_agents_dir completions_dir; do
        eval 'value=$'"$key"
        [ -n "$value" ] || continue
        absolute_path "$value"; value=$resolved; eval "$key=\$value"
    done
    codex_agents_destination
    completion_destination
    bundle_dir=$data_dir/bundle
    [ ! -L "$state_dir/uninstall.tsv" ] || die "refusing symlink: $state_dir/uninstall.tsv"
    [ ! -e "$state_dir/uninstall.tsv" ] || [ -f "$state_dir/uninstall.tsv" ] || die "invalid uninstall inventory: $state_dir/uninstall.tsv"
    backup_dir=$state_dir/backups/$(date +%Y%m%dT%H%M%S)-$$
    cp "$work/source-plan" "$work/plan"
    : > "$work/skip"
    : > "$work/warnings"
    has_components ||
        die 'nothing selected; omit --no-cli or select --skills or --agents'
    if [ -z "$from_repo" ] || [ "$dry_run" = 0 ]; then
        if [ "$plugin_claude" = 1 ]; then
            command -v claude >/dev/null 2>&1 || die '--plugin claude requires Claude Code on PATH'
        fi
        if [ "$plugin_codex" = 1 ]; then
            command -v codex >/dev/null 2>&1 || die '--plugin codex requires Codex on PATH'
        fi
        if [ "$mode" = plugin ] || [ -n "$from_repo" ]; then
            command -v python3 >/dev/null 2>&1 || die 'plugin configuration and local packaging require python3'
        fi
    fi
    plugin_release=$release
    if [ "$mode" = plugin ]; then
        # The marketplace directory must always belong to this installer, even with --force.
        if [ -e "$bundle_dir" ] || [ -L "$bundle_dir" ]; then
            [ ! -L "$bundle_dir" ] && [ -f "$bundle_dir/.leanck-installed" ] ||
                die "$bundle_dir already exists and is not installer-managed; choose another --data-dir"
        fi
        check_destination "$bundle_dir"

    fi
    for host in claude codex; do
        if [ "$host" = claude ]; then
            selected=$skills_claude; skills_dir=$claude_dir/skills
        else
            selected=$skills_codex; skills_dir=$codex_skills
        fi
        if [ "$selected" = 1 ]; then
            for skill in "$plugin"/skills/*; do check_destination "$skills_dir/${skill##*/}"; done
        fi
    done
    if [ "$agents_claude" = 1 ]; then check_agents "$plugin/agents" "$claude_dir/agents" '*.md'; fi
    if [ "$agents_codex" = 1 ]; then check_agents "$plugin/codex/agents" "$codex_agent_path" '*.toml'; fi
    if [ "$mode" = plugin ]; then
        check_destination "$data_dir/agents"
        if [ "$plugin_codex" = 1 ]; then
            check_agents "$plugin/codex/agents" "$codex_agent_path" '*.toml' "$data_dir/agents"
        fi

    fi
    extra_locations check
    plan_codex_agent_migration
    if [ "$cli" = 1 ] && [ -e "$bin_dir/leanck" ] && [ "$last_binary_path" = "$bin_dir/leanck" ] &&
        [ "$last_binary_digest" != "$(digest_file "$bin_dir/leanck")" ]; then
        conflict "$bin_dir/leanck"
    fi
    if [ "$cli" = 1 ] && [ "$completions" != none ]; then
        if [ -e "$completion_dir" ] || [ -L "$completion_dir" ]; then
            [ -d "$completion_dir" ] || die "completions directory is not a directory: $completion_dir; choose another path in advanced config or with --completions-dir"
        else
            warn_skipped_option "Completions directory does not exist: $completion_dir; skipping shell completions. Choose an existing directory in advanced config or with --completions-dir."
            printf '%s\n' "$completion_path" >> "$work/skip"
            plan "Skip shell completions (directory missing): $completion_dir"
        fi
    fi
    if [ "$cli" = 1 ] && [ "$completions" != none ] && { [ -e "$completion_path" ] || [ -L "$completion_path" ]; }; then
        if [ "$last_completions_path" != "$completion_path" ] ||
            [ "$last_completions_digest" != "$(digest_file "$completion_path")" ]; then
            conflict "$completion_path"
        fi
    fi
    if [ "$cli" = 1 ] && [ "$add_to_path" = 1 ]; then
        case "$shell" in fish|bash|zsh) ;; *) die '--add-to-path needs --shell fish, bash, or zsh' ;; esac
        [ ! -L "$path_file" ] || die "refusing symlink profile: $path_file"
        if [ -e "$path_file" ]; then
            [ -f "$path_file" ] || die "shell profile is not a regular file: $path_file"
            awk '
                /^# >>> leanck PATH >>>$/ {if(open || count) exit 1; open=1; count++; next}
                /^# <<< leanck PATH <<<$/ {if(!open) exit 1; open=0}
                END {if(open) exit 1}
            ' "$path_file" || die "unbalanced leanck PATH markers in $path_file; repair the block before retrying"
        else
            warn_skipped_option "Shell profile does not exist: $path_file; skipping PATH update. Choose an existing file in advanced config or with --path-file."
            printf '%s\n' "$path_file" >> "$work/skip"
            plan "Skip PATH update (shell profile missing): $path_file"
        fi
    fi
    planning=1
    if [ -n "$from_repo" ] && [ "$dry_run" = 0 ]; then
        if [ "$cli" = 1 ]; then plan "Would build leanck from $from_repo (locked native release build)"; fi
        plan "Would package local skills and agents from $from_repo"
    fi
    apply_install
    planning=0
    plan "Installer choices and replacement backups: $state_dir"
}

main() {
    set -eu
    umask 077
    cli=1 force=0 dry_run=0 release_unavailable=0 version=latest mode=plugin mode_requested=''
    skills_claude=0 skills_codex=0 agents_claude=0 agents_codex=0 plugin_claude=0 plugin_codex=0
    yes=0 json=0 quiet=0 verbose=0 no_color=0 no_pager=0 interactive=0 planning=0
    committed=0 host_started=0 sequence=0 outcome=failed error='' progress_pid='' form_active=0
    work='' pending='' installed_version='' timeout=300 add_to_path=0
    from_repo='' extra_skills='' extra_claude_agents='' extra_codex_agents=''
    codex_agents_dir='' completions_dir=''
    active_child='' command_watchdog=''
    last_binary_path='' last_binary_digest='' last_completions_path='' last_completions_digest=''
    colors
    trap cleanup 0
    trap cancel INT
    trap 'exit 143' TERM HUP
    # Pre-scan global presentation/state flags before diagnostics or loading saved choices.
    for option in "$@"; do
        case "$option" in --json) json=1 ;; --no-color) no_color=1 ;; -q|--quiet) quiet=1 ;; esac
    done
    colors
    [ -n "${HOME:-}" ] || die 'HOME must be set'
    standalone_bin_dir=$HOME/.local/bin plugin_bin_dir=''
    bin_dir=$standalone_bin_dir
    data_dir=${XDG_DATA_HOME:-$HOME/.local/share}/leanck
    state_dir=${XDG_STATE_HOME:-$HOME/.local/state}/leanck/installer
    claude_dir=${CLAUDE_CONFIG_DIR:-$HOME/.claude}
    codex_dir=${CODEX_HOME:-$HOME/.codex}
    codex_skills=$HOME/.agents/skills
    # Some /bin/sh implementations invent an unexported SHELL from the account's
    # login shell. Only an inherited SHELL identifies the caller's shell here.
    shell=$(env | sed -n 's/^SHELL=//p'); shell=${shell##*/}; shell=${shell:-sh}
    default_completions "$shell"
    fish_completions_dir=${XDG_CONFIG_HOME:-$HOME/.config}/fish/completions
    case "$shell" in
        fish) path_file=${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish ;;
        zsh) path_file=$HOME/.zshrc ;;
        *) path_file=$HOME/.bashrc ;;
    esac
    release_url=https://github.com/danlyng/leanck/releases
    original_path=$PATH
    state_next=0
    for option in "$@"; do
        if [ "$state_next" = 1 ]; then state_dir=$option; state_next=0; fi
        [ "$option" != --state-dir ] || state_next=1
    done
    absolute_path "$state_dir"; state_dir=$resolved
    # Help must work even when saved state is broken or inaccessible.
    for option in "$@"; do case "$option" in -h|--help) json=0; usage; outcome=help; exit 0 ;; esac; done
    if [ "$#" = 1 ] && [ "$1" = help ]; then json=0; usage; outcome=help; exit 0; fi
    load_state
    # An explicitly selected Codex home takes precedence over remembered destinations.
    if [ -n "${CODEX_HOME:-}" ]; then codex_dir=$CODEX_HOME; fi
    shell_changed=0 profile_set=0 completions_set=0 bin_dir_set=0 path_set=0 cli_explicit=0 version_set=0
    detect_hosts
    plugin_claude=$detected_claude plugin_codex=$detected_codex
    skills_explicit=0 agents_explicit=0 plugin_explicit=0
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --skills|--agents|--plugin)
                need_value "$@"; key=${1#--}; eval 'explicit=$'"${key}_explicit"
                if [ "$explicit" = 0 ] || [ "$2" = none ]; then eval "${key}_claude=0; ${key}_codex=0"; fi
                eval "${key}_explicit=1"
                select_hosts "$key" "$2"
                if [ "$key" = plugin ]; then
                    mode_requested=plugin; [ "$2" != none ] || mode_requested=standalone
                fi
                shift 2 ;;
            --all) mode_requested=plugin; cli=1; cli_explicit=1; plugin_claude=1; plugin_codex=1; plugin_explicit=1; shift ;;
            --standalone) mode_requested=standalone; shift ;;
            --cli) cli=1; cli_explicit=1; shift ;;
            --no-cli) cli=0; cli_explicit=1; shift ;;
            --dry-run) dry_run=1; shift ;;
            --force) force=1; shift ;;
            -y|--yes) yes=1; shift ;;
            --json) json=1; shift ;;
            --no-color) no_color=1; shift ;;
            --no-pager) no_pager=1; shift ;;
            -q|--quiet) quiet=1; shift ;;
            -v|--verbose) verbose=$((verbose + 1)); shift ;;
            -v*) short=${1#-}
                case "$short" in *[!v]*) die "unknown option: $1; see --help" ;; esac
                verbose=$((verbose + ${#short})); shift ;;
            --add-to-path) add_to_path=1; path_set=1; shift ;;
            --no-add-to-path) add_to_path=0; path_set=1; shift ;;
            --extra-skills-dir|--extra-claude-agents-dir|--extra-codex-agents-dir)
                need_value "$@"; key=$(printf '%s' "${1#--}" | tr - _); key=${key%_dir}
                append_location "$key" "$2"; shift 2 ;;
            --from-repo|--version|--bin-dir|--data-dir|--state-dir|--claude-dir|--codex-dir|--codex-skills-dir|--codex-agents-dir|--fish-completions-dir|--completions-dir|--completions|--shell|--path-file|--release-url|--timeout)
                need_value "$@"; valid_text "$2"
                case "$1" in --codex-skills-dir) key=codex_skills ;; *) key=$(printf '%s' "${1#--}" | tr - _ ) ;; esac
                value=$2; eval "$key=\$value"
                case "$key" in shell) shell_changed=1 ;; path_file) profile_set=1 ;; completions) completions_set=1 ;; bin_dir) bin_dir_set=1 ;; version) version_set=1 ;; esac
                shift 2 ;;
            *) die "unknown option: $1; see --help" ;;
        esac
    done
    if [ -n "$mode_requested" ]; then mode=$mode_requested
    elif [ "$skills_explicit$agents_explicit" != 00 ] || [ "$cli_explicit:$cli" = 1:0 ]; then mode=standalone; fi
    if [ "$mode" = plugin ] && [ "$cli_explicit" = 0 ]; then cli=1; fi
    if [ "$bin_dir_set" = 0 ]; then bin_dir=$standalone_bin_dir; fi
    if [ "$shell_changed" = 1 ]; then
        if [ "$completions_set" = 0 ]; then default_completions "$shell"; fi
        if [ "$profile_set" = 0 ]; then
            case "$shell" in fish) path_file=${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish ;;
                zsh) path_file=$HOME/.zshrc ;; *) path_file=$HOME/.bashrc ;; esac
        fi
    fi
    colors
    if [ -n "$from_repo" ]; then
        [ "$version_set" = 0 ] || die '--version selects a published release; omit it with --from-repo'
        absolute_path "$from_repo"; from_repo=$resolved
        [ -f "$from_repo/crates/leanck-cli/Cargo.toml" ] &&
            [ -d "$from_repo/leanck-skills" ] && [ -d "$from_repo/leanck-subagents" ] ||
            die "not a leanck source checkout: $from_repo; pass --from-repo /path/to/leanck"
        if [ "$dry_run" = 0 ]; then
            for source_file in Cargo.toml Cargo.lock packaging/package_local.py packaging/package_release.py packaging/package_plugin.py; do
                [ -f "$from_repo/$source_file" ] || die "local installation needs $from_repo/$source_file; use a complete, current source checkout"
            done
        fi
    fi
    if [ "$yes" = 0 ] && [ "$json" = 0 ] && [ -t 2 ]; then
        if ( : </dev/tty ) 2>/dev/null; then exec 3<>/dev/tty; interactive=1; fi
    fi
    if [ "$yes" = 0 ] && [ "$dry_run" = 0 ] && [ "$interactive" = 0 ]; then
        die 'approval is required; rerun in a terminal, use --yes for automation, or --dry-run to preview'
    fi
    if [ "$interactive" = 0 ]; then
        if [ "$mode" = plugin ]; then
            [ "$cli" = 1 ] || die 'plugin mode includes the executable; omit --no-cli or use --standalone'
        else
            has_components ||
                die 'nothing selected; omit --no-cli or select --skills or --agents'
        fi
    fi
    say "leanck-installer $INSTALLER_VERSION"
    if [ "$dry_run" = 1 ]; then say 'Dry run: approval only completes the preview; nothing will be installed.'; fi
    release_unavailable=0
    absolute_path "$state_dir"; state_dir=$resolved
    absolute_path "$bin_dir"; bin_dir=$resolved
    absolute_path "$data_dir"; data_dir=$resolved
    absolute_path "$claude_dir"; claude_dir=$resolved
    absolute_path "$codex_dir"; codex_dir=$resolved
    absolute_path "$codex_skills"; codex_skills=$resolved
    absolute_path "$fish_completions_dir"; fish_completions_dir=$resolved
    absolute_path "$path_file"; path_file=$resolved
    bundle_dir=$data_dir/bundle
    case "$completions" in bash|zsh|fish|powershell|elvish|none) ;; *) die '--completions expects bash, zsh, fish, powershell, elvish, or none' ;; esac
    case "$shell" in fish|bash|zsh) ;; *) [ "$add_to_path" = 0 ] || die '--add-to-path needs --shell fish, bash, or zsh' ;; esac
    case "$timeout" in ''|*[!0-9]*|0) die '--timeout must be a positive number of seconds' ;; esac
    case "$release_url" in https://*) ;; *) die '--release-url must use HTTPS' ;; esac
    release_url=${release_url%/}
    case "$version" in
        latest) release_path=latest/download ;;
        *) version=${version#v}
           case "$version" in ''|*[!0-9A-Za-z.+-]*) die 'invalid release version' ;; esac
           release_path=download/v$version ;;
    esac
    prerequisites=mktemp
    [ -n "$from_repo" ] || prerequisites='curl tar mktemp'
    for command in $prerequisites; do
        command -v "$command" >/dev/null 2>&1 || die "required command is missing: $command"
    done
    if command -v sha256sum >/dev/null 2>&1; then
        hash_command=sha256sum
    elif command -v shasum >/dev/null 2>&1; then
        hash_command=shasum
    else
        die 'SHA-256 verification requires sha256sum or shasum'
    fi
    system=$(uname -s)
    machine=$(uname -m)
    case "$system" in
        Darwin) platform=apple-darwin ;;
        Linux) platform=unknown-linux-musl ;;
        *) die "unsupported OS: $system; use a Linux/WSL or macOS host" ;;
    esac
    case "$machine" in
        x86_64|amd64) arch=x86_64 ;;
        arm64|aarch64) arch=aarch64 ;;
        *) die "unsupported architecture: $machine" ;;
    esac

    if [ -n "$from_repo" ]; then
        say "Checking local source inventory in $from_repo"
    elif [ "$dry_run" = 1 ]; then
        say "Dry run: checking leanck for $arch-$platform (temporary downloads only)"
    else
        say "Preparing leanck for $arch-$platform"
    fi
    work=$(mktemp -d "${TMPDIR:-/tmp}/leanck-install.XXXXXX")
    : > "$work/plan"; : > "$work/skip"; : > "$work/journal"; : > "$work/sources.tsv"
    backup_dir=$state_dir/backups/$(date +%Y%m%dT%H%M%S)-$$
    if [ -n "$from_repo" ]; then
        prepare_repo_source
    else
        archive=leanck-$arch-$platform.tar.gz
        download "$release_url/$release_path/$archive" "$work/$archive"
        if [ "$release_unavailable" = 0 ]; then
            download "$release_url/$release_path/$archive.sha256" "$work/checksum"
        fi
        if [ "$release_unavailable" = 1 ]; then
            prepare_preview
        else
            # Hash only the expected archive: never accept filenames/options from the checksum file.
            expected=$(awk 'NR == 1 {print $1}' "$work/checksum")
            case "$expected" in ''|*[!0-9a-fA-F]*) die 'invalid SHA-256 checksum' ;; esac
            [ "${#expected}" = 64 ] || die 'invalid SHA-256 checksum'
            if [ "$hash_command" = sha256sum ]; then
                actual=$(sha256sum "$work/$archive")
            else
                actual=$(shasum -a 256 "$work/$archive")
            fi
            [ "$expected" = "${actual%% *}" ] || die 'SHA-256 checksum mismatch; nothing installed'
            # Release bundles contain regular files/directories only, under a single leanck/ root.
            tar -tzf "$work/$archive" > "$work/members"
            while IFS= read -r member; do
                case "$member" in leanck|leanck/*) ;; *) die "unsafe archive member: $member" ;; esac
                case "/$member/" in */../*|*/./*) die "unsafe archive member: $member" ;; esac
            done < "$work/members"
            tar -tvzf "$work/$archive" > "$work/types"
            while IFS= read -r entry; do
                case "$entry" in [-d]*) ;; *) die 'archive contains a link or special file' ;; esac
            done < "$work/types"
            tar -xzf "$work/$archive" -C "$work"
            release=$work/leanck
            plugin=$release/plugin
            [ -f "$release/.leanck-release" ] && [ -x "$plugin/bin/leanck" ] &&
                [ -f "$plugin/.claude-plugin/plugin.json" ] && [ -f "$plugin/.codex-plugin/plugin.json" ] ||
                die 'incomplete release bundle'
            installed_version=$(cat "$release/.leanck-release")
            if [ "$version" != latest ]; then
                [ "$installed_version" = "v$version" ] || die 'release bundle version does not match requested version'
            fi
        fi
    fi

    if [ "$quiet" = 0 ] || [ "$interactive" = 1 ]; then
        printf '\n%sWelcome to the leanck %s installer%s\n' "$bold" "$installed_version" "$reset" >&2
        printf 'Detected coding agents: %s\n' "$detected_agents" >&2
    fi
    if [ "$interactive" = 1 ]; then
        command -v bash >/dev/null 2>&1 || die 'interactive prompts require bash; install bash or use --yes with explicit flags'
        choose 'Install as plugin or standalone?' "$mode" 'plugin standalone' 'Plugin (recommended)|Standalone'
        mode=$reply
    fi
    if [ "$mode" = plugin ]; then
        [ "$cli" = 1 ] || die 'plugin mode includes the executable; omit --no-cli or use --standalone'
        skills_claude=0 skills_codex=0 agents_claude=0 agents_codex=0
        if [ "$bin_dir_set" = 0 ]; then
            bin_dir=${plugin_bin_dir:-$data_dir/bin}
            absolute_path "$bin_dir"; bin_dir=$resolved
        fi
        [ "$path_set" = 1 ] || add_to_path=1
        if [ "$interactive" = 1 ]; then
            configure_completions
            configure_path
        fi
    else
        plugin_claude=0 plugin_codex=0
        if [ "$interactive" = 1 ]; then configure_standalone; fi
        absolute_path "$bin_dir"; bin_dir=$resolved
        absolute_path "$claude_dir"; claude_dir=$resolved
        absolute_path "$codex_dir"; codex_dir=$resolved
        absolute_path "$codex_skills"; codex_skills=$resolved
        absolute_path "$fish_completions_dir"; fish_completions_dir=$resolved
        absolute_path "$path_file"; path_file=$resolved
        has_components ||
            die 'nothing selected; omit --no-cli or select --skills or --agents'
    fi
    cp "$work/plan" "$work/source-plan"
    prepare_plan
    if [ "$interactive" = 1 ]; then
        if [ "$mode" = plugin ]; then confirmation='Installing `leanck` as a plugin. Continue?'
        else confirmation='Installing `leanck` as standalone. Continue?'; fi
        while :; do
            while IFS= read -r skipped_warning; do warn "$skipped_warning"; done < "$work/warnings"
            choose "$confirmation" cancel 'cancel continue edit' 'Cancel|Continue|Edit advanced config...'
            case "$reply" in
                continue) break ;;
                edit)
                    previous_config=$(config_snapshot)
                    edit_advanced_config
                    if [ "$previous_config" != "$(config_snapshot)" ]; then prepare_plan; fi
                    [ "$verbose" = 0 ] || show_plan ;;
                cancel) outcome=cancelled; say 'Cancelled; nothing installed.'; return 0 ;;
            esac
        done
    elif [ "$quiet" = 0 ]; then
        inspect_config
        show_plan
    fi
    if [ "$dry_run" = 1 ]; then
        outcome=preview
        say 'Dry run complete; no installed files or host configuration changed. Downloaded binaries and host CLIs were not executed.'
        return 0
    fi
    if [ "$plugin_codex" = 1 ]; then
        run_bounded "$timeout" env CODEX_HOME="$codex_dir" codex plugin add --help >/dev/null 2>&1 || die 'update Codex to a version with codex plugin add'
    fi
    if [ -n "$from_repo" ]; then build_repo_source; fi
    [ "$verbose" -lt 2 ] || say 'Applying file changes, then registering selected host plugins.'
    if [ -f "$release/.leanck-update-protocol" ] && [ -x "$plugin/bin/leanck" ]; then
        mkdir -p "$state_dir"
        [ ! -L "$state_dir/maintenance.lock" ] || die 'refusing symlink maintenance lock'
        exec 9<>"$state_dir/maintenance.lock"
        run_bounded "$timeout" "$plugin/bin/leanck" _update lock-fd --fd 9 --binary "$bin_dir/leanck" --binary "$bundle_dir/plugin/bin/leanck" >&2 || die 'installation is busy; close leanck/MCP clients and stop daemons before retrying'
    fi
    apply_install
    save_state
    committed=1
    outcome=installed
    if [ "$plugin_claude$plugin_codex" != 00 ]; then
        outcome=partial
        register_plugins
        if [ -f "$release/.leanck-update-protocol" ] && [ "$cli" = 1 ] && ! skipped "$bin_dir/leanck"; then
            pending=$(mktemp "$state_dir/.leanck-install.XXXXXX")
            run_bounded "$timeout" "$bin_dir/leanck" _update receipt --state-dir "$state_dir" --binary "$bin_dir/leanck" --registration-complete > "$pending" || die 'could not record completed plugin registration'
            commit "$pending" "$state_dir/receipt.json"
        fi
        outcome=installed
    elif [ "$mode" = plugin ]; then
        say "Plugin files are ready in $bundle_dir for registration when an agent host is installed."
    fi
    if [ -d "$backup_dir" ]; then say "Replacement backups: $backup_dir"; fi
    if [ "$cli" = 1 ]; then
        case ":$original_path:" in
            *":$bin_dir:"*) ;;
            *) if [ "$add_to_path" = 0 ] || skipped "$path_file"; then say "Add $bin_dir to PATH in your shell and agent host."
               else say 'Open a new shell to load the PATH change; restart the agent host as needed.'; fi ;;
        esac
        say 'After an upgrade, stop old daemons with: leanck daemons stop --all'
        if [ "$completions" != none ] && ! skipped "$completion_path"; then
            say "Shell completions ($completions): $completion_path"
            case "$completions" in
                zsh) say 'Include the completions directory in fpath before calling compinit.' ;;
                powershell) say 'Load the completion script from your PowerShell profile.' ;;
                elvish) say 'Load the completion module with use leanck.' ;;
            esac
        fi
    fi
    if [ "$cli" = 0 ] && ! command -v leanck >/dev/null 2>&1; then
        say 'Standalone skills and agents require an existing leanck on PATH.'
    fi
    if [ "$quiet" = 0 ]; then printf '%sDone.%s\n' "$bold" "$reset" >&2; fi
    say 'Live queries require the Lean project toolchain and Lake (usually through elan).'
}

main "$@"
