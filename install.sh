#!/bin/sh
# Install leanck: download and verify a release, then run its own installer,
# `leanck _install`, which asks, plans, and applies. Keep this script usable with
# curl | sh: everything runs inside main, so a truncated download installs nothing.

say() { [ "$quiet" = 1 ] || printf '%s[INFO]%s %s\n' "$cyan" "$reset" "$*" >&2; }
die() { error=$*; printf '%s [ERR]:%s %s\n' "$red" "$reset" "$*" >&2; exit 1; }
json_string() { printf '"%s"' "$(printf '%s' "$1" | tr -d '\000-\037' | sed 's/[\\"]/\\&/g')"; }
need_value() { case "$2" in ''|--*) die "$1 needs a value; see --help" ;; esac; }
cleanup() {
    status=$?
    trap - 0
    # The child reports its own outcome; report ours if it never ran or died silently.
    if [ "$json" = 1 ] && [ -n "$work" ] && [ -s "$work/outcome.json" ]; then
        cat "$work/outcome.json"
    elif [ "$json" = 1 ]; then
        [ -n "$error" ] || [ "$status" = 0 ] || error="Installer exited with status $status; rerun with --verbose for details"
        printf '{"status":%s,"version":%s,"message":%s,"actions":[]}\n' \
            "$(json_string "${outcome:-failed}")" "$(json_string "$version")" "$(json_string "$error")"
    fi
    [ -z "$work" ] || rm -rf "$work"
    exit "$status"
}
usage() {
    cat <<'EOF'
Usage: curl -fsSL https://lean4.party/leanck-install.sh | sh -s -- [OPTIONS]
       sh install.sh [OPTIONS]

Download a leanck release, verify its SHA-256 checksum, and run its installer.

Bootstrap options:
  -y, --yes            Approve without prompts (required without a terminal)
      --dry-run        Preview the installation without changing anything
      --json           Print one JSON status object on stdout; disables prompts
      --version TAG    Release to install (default: latest, or the saved pin)
      --release-url URL  Release mirror: https://, or file:// for a local copy
      --from-repo DIR  Build leanck from a source checkout with cargo and install it
      --timeout SECS   Download and command timeout (default: 300)
  -h, --help           Show this help

All other options choose what to install and where (plugin or --standalone mode,
hosts, skills, agents, completions, directories) and go to the release's installer;
`leanck _install --help` lists them.

Examples:
  curl -fsSL https://lean4.party/leanck-install.sh | sh
  sh install.sh --yes --all
  sh install.sh --standalone --skills both --skill leanck
  sh install.sh --from-repo .
EOF
}
fetch() {
    case "$1" in
        file://*) [ -f "${1#file://}" ] || return 4; cp "${1#file://}" "$2" ;;
        *) code=$(curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
                --connect-timeout 15 --max-time "$timeout" --retry 2 --write-out '%{http_code}' \
                --output "$2" "$1" 2> "$2.error") && return 0
           [ "$code" = 404 ] && return 4; cat "$2.error" >&2; return 1 ;;
    esac
}
download() {
    fetch "$1" "$2" && return 0
    [ $? = 4 ] || die "download failed: $1; check the network connection or --release-url"
    [ "$version" = latest ] && die "no release found at $release_url"
    die "release v$version not found"
}

main() {
    error='' outcome='' work='' version=latest release_url=https://github.com/danlyng/leanck/releases mirror=0
    yes=0 dry_run=0 json=0 quiet=0 color=1 from_repo='' timeout=300 state_dir='' pinned=0 cyan='' red='' reset=''
    if [ "$#" = 1 ] && [ "$1" = help ]; then usage; exit 0; fi
    for arg in "$@"; do
        case "$arg" in -h|--help) usage; exit 0 ;; --json) json=1 ;; -q|--quiet) quiet=1 ;; --no-color) color=0 ;; esac
    done
    if [ "$color" = 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != dumb ] && [ -t 2 ] && [ "$json" = 0 ]; then
        cyan=$(printf '\033[36m') red=$(printf '\033[31m') reset=$(printf '\033[0m')
    fi
    trap cleanup 0
    trap 'exit 130' INT
    trap 'exit 143' TERM HUP
    # Keep our flags; pass every other argument, with its value, through in order.
    n=$#
    while [ "$n" -gt 0 ]; do
        arg=$1; shift; n=$((n - 1))
        case "$arg" in --version=*|--release-url=*|--from-repo=*|--timeout=*|--state-dir=*)
            set -- "${arg#*=}" "$@"; n=$((n + 1)) arg=${arg%%=*} ;;
        esac
        value=''
        [ "$n" = 0 ] || value=$1
        case "$arg" in
            -y|--yes) yes=1; set -- "$@" --yes ;;
            --dry-run) dry_run=1; set -- "$@" --dry-run ;;
            --version|--release-url|--from-repo|--timeout|--state-dir|--plugin|--skills|--skill|--agents|\
            --completions|--bin-dir|--data-dir|--claude-dir|--codex-dir|--codex-skills-dir|--codex-agents-dir|\
            --completions-dir|--fish-completions-dir|--extra-skills-dir|--extra-claude-agents-dir|\
            --extra-codex-agents-dir|--shell|--path-file)
                need_value "$arg" "$value"; shift; n=$((n - 1))
                case "$arg" in
                    --version) version=${value#v} pinned=1 ;;
                    --release-url) release_url=${value%/} mirror=1; continue ;;
                    --from-repo) from_repo=$value; continue ;;
                    --timeout) timeout=$value ;;
                    --state-dir) state_dir=$value ;;
                esac
                set -- "$@" "$arg" "$value" ;;
            *) set -- "$@" "$arg" ;;
        esac
    done
    [ -n "${HOME:-}" ] || die 'HOME must be set'
    case "$timeout" in ''|*[!0-9]*) die '--timeout must be a positive number of seconds' ;; esac
    [ "$timeout" -gt 0 ] || die '--timeout must be a positive number of seconds'
    case "$version" in ''|*[!0-9A-Za-z.+-]*) die 'invalid release version' ;; esac
    # The installer records an HTTPS mirror for `leanck update`; a file:// copy serves this run only.
    case "$release_url" in https://*) [ "$mirror" = 0 ] || set -- "$@" --release-url "$release_url" ;;
        file:///*) ;; *) die '--release-url must use HTTPS (or file:// for a local mirror)' ;; esac
    # Approval comes before any network access: without --yes, a terminal must be there to ask.
    interactive=0
    if [ "$yes" = 0 ] && [ "$json" = 0 ] && [ -t 2 ] && (: < /dev/tty) 2>/dev/null; then interactive=1; fi
    if [ "$yes" = 0 ] && [ "$dry_run" = 0 ] && [ "$interactive" = 0 ]; then
        die 'approval is required; rerun in a terminal, use --yes for automation, or --dry-run to preview'
    fi
    work=$(mktemp -d "${TMPDIR:-/tmp}/leanck-install.XXXXXX") || die 'cannot create a temporary directory'
    if [ -n "$from_repo" ]; then build_from_repo "$@"; else install_release "$@"; fi
}

install_release() {
    if [ "$pinned" = 0 ]; then  # A remembered pin selects the release unless --version overrides it.
        state=${state_dir:-${XDG_STATE_HOME:-$HOME/.local/state}/leanck/installer}
        case "$state" in /*) ;; "~"/*) state=$HOME/${state#"~"/} ;; *) state=$PWD/$state ;; esac
        saved=$(LC_ALL=C awk -F '\t' '$1 == "version" { v = $2 } END { print v }' "$state/selections.tsv" 2>/dev/null)
        case "$saved" in ''|latest) ;; *) version=${saved#v} ;; esac
    fi
    case "$(uname -s)" in Darwin) platform=apple-darwin ;; Linux) platform=unknown-linux-musl ;;
        *) die "unsupported OS: $(uname -s); use a Linux/WSL or macOS host" ;; esac
    case "$(uname -m)" in x86_64|amd64) arch=x86_64 ;; arm64|aarch64) arch=aarch64 ;;
        *) die "unsupported architecture: $(uname -m)" ;; esac
    if command -v sha256sum >/dev/null 2>&1; then hasher='sha256sum'; elif command -v shasum >/dev/null 2>&1; then
        hasher='shasum -a 256'; else die 'SHA-256 verification requires sha256sum or shasum'; fi
    need=tar; case "$release_url" in https:*) need="curl tar" ;; esac
    for command in $need; do command -v "$command" >/dev/null 2>&1 || die "required command is missing: $command"; done
    archive=leanck-$arch-$platform.tar.gz path=download/v$version
    [ "$version" = latest ] && path=latest/download
    say "Downloading $archive from $release_url/$path"
    mkdir "$work/download" "$work/release"
    download "$release_url/$path/$archive" "$work/download/$archive"
    download "$release_url/$path/$archive.sha256" "$work/download/checksum"
    # Hash only the expected archive; never take names or options from the checksum file.
    expected=$(awk 'NR == 1 { print $1 }' "$work/download/checksum" | tr 'A-F' 'a-f')
    case "$expected" in *[!0-9a-f]*) die 'invalid SHA-256 checksum' ;; esac
    [ "${#expected}" = 64 ] || die 'invalid SHA-256 checksum'
    actual=$($hasher "$work/download/$archive" | awk '{ print $1 }' | tr 'A-F' 'a-f')
    [ "$expected" = "$actual" ] || die 'SHA-256 checksum mismatch; nothing installed'
    # Releases hold only regular files and directories under a single leanck/ root.
    tar -tzf "$work/download/$archive" > "$work/members" || die 'cannot read the release archive'
    unsafe=$(grep -E -v '^leanck(/|$)' "$work/members"; grep -E '(^|/)\.\.?(/|$)' "$work/members")
    [ -z "$unsafe" ] || die "unsafe archive member: $(printf '%s\n' "$unsafe" | head -n 1)"
    if tar -tvzf "$work/download/$archive" | grep -q -v '^[-d]'; then die 'archive contains a link or special file'; fi
    tar -xzf "$work/download/$archive" -C "$work/release" || die 'cannot extract the release archive'
    run_installer "$work/release/leanck" "$@"
}

build_from_repo() {
    [ "$pinned" = 0 ] || die '--version selects a published release; omit it with --from-repo'
    repo=$(CDPATH='' cd -- "$from_repo" 2>/dev/null && pwd) || repo=''
    for file in Cargo.toml Cargo.lock crates/leanck-cli/Cargo.toml xtask/Cargo.toml leanck-skills/ leanck-subagents/; do
        [ -e "${repo:-/nonexistent}/$file" ] ||
            die "not a leanck source checkout: $from_repo; pass --from-repo /path/to/leanck"
    done
    command -v cargo >/dev/null 2>&1 || die 'local builds require cargo on PATH'
    if [ "$yes" = 0 ]; then  # Nothing from the checkout runs before this answer.
        [ "$interactive" = 1 ] || die '--from-repo builds before previewing; add --yes to approve the build'
        printf "Build leanck from %s with cargo? This runs the repository's build scripts. [y/N]: " "$repo" >&2
        read -r answer < /dev/tty || answer=''
        case "$answer" in y|Y|yes|YES) ;; *) outcome=cancelled; say 'Cancelled; nothing built or installed.'; exit 0 ;; esac
    fi
    say "Building leanck from $repo"
    (cd "$repo" && cargo build --release --locked -p leanck-cli --target-dir "$repo/target") >&2 ||
        die 'local build failed; check the build output above'
    built=$("$repo/target/release/leanck" --version) || die 'the built leanck does not run'
    say 'Packaging local skills, agents, and plugin configuration'
    (cd "$repo" && cargo xtask package-local --bin-dir "$repo/target/release" --output "$work/leanck" \
        --version "${built#leanck }") >&2 || die 'could not package local source; no installation files were changed'
    run_installer "$work/leanck" --from-repo "$repo" "$@"
}

run_installer() {
    release=$1; shift
    protocol=$(head -n 1 "$release/.leanck-update-protocol" 2>/dev/null)
    case "$protocol" in ''|*[!0-9]*) protocol=0 ;; esac
    if [ "$protocol" -lt 3 ]; then
        tag=$(cat "$release/.leanck-release" 2>/dev/null)
        die "${tag:-v$version} predates this installer; use the install.sh attached to that release"
    fi
    # A child, not exec: the exit trap still removes the download and reports JSON.
    if [ "$json" = 1 ]; then
        "$release/plugin/bin/leanck" _install --release-dir "$release" "$@" > "$work/outcome.json"
    else
        "$release/plugin/bin/leanck" _install --release-dir "$release" "$@"
    fi
}

main "$@"
