#!/bin/sh
# Bootstrap only: verify the native installer, which validates and publishes parts.
set -eu
fail() { printf '%s\n' "hive install: $*" >&2; exit 1; }
diagnostic() {
    # Fixed phase codes only. No argv, source URLs, environment values or native text.
    printf '{"event":"bootstrap.%s"}\n' "$1" >&2 || :
}
usage() {
    printf '%s\n' \
        'Usage: install.sh [--from DIR|HTTPS_URL] [--prefix DIR] [--core] [--comb]' \
        '       [--standalone-comb] [--no-setup] [--yes] [--setup-FLAG VALUE ... | -- SETUP_ARGS...]' \
        '       [--uninstall [--dry-run] [--purge [--purge-comb-root PATH]]]' \
        'Purge scope: --config FILE --state-dir DIR --comb-root DIR --bridge-file FILE' \
        'Examples:' \
        '  sh install.sh --from /private/assets --prefix /private/hive --comb --no-setup' \
        '  sh install.sh --prefix /private/hive --uninstall --yes' \
        '  sh install.sh --prefix /private/hive --uninstall --purge --dry-run'
}
input_error() {
    # Only trusted static metadata is passed here; never interpolate user argv.
    printf '{"error":{"code":"%s","field":"%s","message":"%s","expected":"%s","examples":["sh install.sh --from /private/assets --prefix /private/hive --comb --no-setup","sh install.sh --prefix /private/hive --uninstall --purge --dry-run"]}}\n' "$1" "$2" "$3" "$4"
    printf '%s: %s Expected: %s\n' "$2" "$3" "$4" >&2
    usage >&2
    exit 2
}
mark() {
    case " $seen " in *" $1 "*) input_error conflicting_arguments "$1" 'Option repeated.' 'specify each installer option once';; esac
    seen="$seen $1"
}
value() {
    [ "$#" -ge 2 ] || input_error missing_argument "$1" 'Missing option value.' 'a nonempty value after the option'
    case "$2" in ''|-*) input_error invalid_value "$1" 'Missing or invalid option value.' 'a nonempty value which is not another option';; esac
}
source_dir=''
prefix=''
seen=''
core=false
comb=false
standalone=false
uninstall=false
no_setup=false
purge=false
dry_run=false
scope=false
setup=false
parse_arguments() {
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --from|--prefix)
                mark "$1"; value "$@"
                case "$1" in --from) source_dir=$2;; --prefix) prefix=$2;; esac
                shift 2;;
            --core) mark "$1"; core=true; shift;;
            --comb) mark "$1"; comb=true; shift;;
            --standalone-comb) mark "$1"; standalone=true; shift;;
            --uninstall) mark "$1"; uninstall=true; shift;;
            --no-setup) mark "$1"; no_setup=true; shift;;
            --yes) mark "$1"; shift;;
            --purge) mark "$1"; purge=true; shift;;
            --dry-run) mark "$1"; dry_run=true; shift;;
            --purge-comb-root|--config|--state-dir|--comb-root|--bridge-file)
                mark "$1"; value "$@"; scope=true
                shift 2;;
            --setup-*=*) setup=true; shift;;
            --setup-*)
                setup=true
                shift
                if [ "$#" -gt 0 ]; then case "$1" in -*) :;; *) shift;; esac; fi;;
            --) [ "$#" -le 1 ] || setup=true; break;;
            -h|--help) usage; exit 0;;
            *) input_error unknown_argument arguments 'Unknown installer option.' 'an installer option listed in usage';;
        esac
    done
}
# A function has its own positional parameters: preserve the exact original argv
# for the native parser, including whitespace, literal dollars and setup flags.
parse_arguments "$@"
if $standalone && $core; then input_error conflicting_arguments --core 'Conflicting builds.' 'choose core or standalone Comb'; fi
if $setup && { $no_setup || $uninstall; }; then input_error conflicting_arguments setup 'Setup arguments would be ignored.' 'omit setup arguments with --no-setup or --uninstall'; fi
if $no_setup && $uninstall; then input_error conflicting_arguments --no-setup 'Option would be ignored.' 'omit --no-setup with --uninstall'; fi
if { $purge || $dry_run; } && ! $uninstall; then input_error missing_argument --uninstall 'Removal mode is required.' 'use --uninstall with --purge or --dry-run'; fi
if $scope && ! $purge; then input_error missing_argument --purge 'Scope paths require purge.' 'use scope paths only with --uninstall --purge'; fi
if [ -z "$prefix" ]; then
    [ -n "${HOME:-}" ] || input_error missing_argument --prefix 'HOME is unavailable.' 'pass an absolute --prefix directory or set HOME'
    prefix="$HOME/.local"
fi
case "$prefix" in /*) :;; *) input_error invalid_path --prefix 'Invalid installation prefix.' 'a nonempty absolute directory';; esac
case "/$prefix/" in */../*) input_error invalid_path --prefix 'Parent traversal is not allowed.' 'an absolute directory without parent traversal';; esac
while :; do
    case "$prefix" in /) break;; */) prefix=${prefix%/};; */.) prefix=${prefix%/.};; *) break;; esac
done
[ -n "$prefix" ] || prefix=/
if [ -L "$prefix" ] || { [ -e "$prefix" ] && [ ! -d "$prefix" ]; }; then
    input_error invalid_path --prefix 'Invalid installation prefix.' 'a directory which is not a leaf symlink'
fi
ancestor=$prefix
while [ ! -d "$ancestor" ]; do
    if [ -e "$ancestor" ] || [ -L "$ancestor" ]; then input_error invalid_path --prefix 'Invalid prefix ancestor.' 'an existing directory ancestor'; fi
    ancestor=${ancestor%/*}
    [ -n "$ancestor" ] || ancestor=/
done
[ -x "$ancestor" ] || input_error invalid_path --prefix 'Cannot access prefix ancestor.' 'a searchable directory ancestor'
validate_source() {
    case "$source_dir" in
        https://*)
            authority=${source_dir#https://}; authority=${authority%%/*}
            authority=${authority%%\?*}; authority=${authority%%\#*}
            authority=${authority##*@}
            case "$authority" in
                \[*) host=${authority#\[}; case "$host" in *\]*) host=${host%%\]*};; *\]:*) host=${host%%\]*};; *) host='';; esac;;
                *) host=${authority%%:*};;
            esac
            case "$host" in ''|*[![:graph:]]*) input_error invalid_value --from 'HTTPS source has no valid host.' 'an HTTPS URL with a nonempty host';; esac;;
        *://*) input_error invalid_value --from 'Unsupported source URL.' 'an HTTPS URL or an existing directory';;
        *) [ -d "$source_dir" ] || input_error invalid_path --from 'Release source is not a directory.' 'an existing directory containing release assets';;
    esac
}
[ -z "$source_dir" ] || validate_source
if $uninstall && [ -z "$source_dir" ] && [ -x "$prefix/bin/hive-install" ]; then
    diagnostic helper_exec
    exec "$prefix/bin/hive-install" "$@"
fi
if [ -z "$source_dir" ]; then
    [ -f "$0" ] || input_error missing_argument --from 'Release source is required.' 'pass --from DIR or an HTTPS release asset base URL when piping into sh'
    source_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
fi
validate_source
cpu=$(uname -m)
case "$cpu" in arm64|aarch64) arch=aarch64;; x86_64) arch=x86_64;; *) fail "Unsupported CPU: $cpu";; esac
os=$(uname -s)
case "$os" in
    Darwin) target="$arch-apple-darwin";;
    Linux)
        target="$arch-unknown-linux-gnu"
        glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null) || fail 'GNU/Linux with glibc 2.35 or newer is required'
        printf '%s\n' "$glibc" | awk '$1=="glibc" {split($2,v,"."); if (v[1]>2 || (v[1]==2 && v[2]>=35)) ok=1} END {exit !ok}' || fail "glibc 2.35 or newer required; found $glibc";;
    *) fail "Unsupported operating system: $os; only macOS and GNU/Linux are supported (Windows: use WSL 2)";;
esac
umask 077
stage=$(mktemp -d "${TMPDIR:-/tmp}/hive-install.XXXXXXXX")
termination=''
cleanup() {
    status=$?
    trap - 0
    trap '' HUP INT TERM
    if ! rm -rf "$stage" 2>/dev/null; then
        diagnostic cleanup_failed
        # Preserve an earlier helper/input failure; a successful run must report
        # its failed cleanup instead of silently leaving a private staging tree.
        [ "$status" -ne 0 ] || status=1
    fi
    trap - HUP INT TERM
    if [ -n "$termination" ]; then
        kill -s "$termination" "$$" || diagnostic signal_delivery_failed
    fi
    exit "$status"
}
interrupted() { termination=$1; diagnostic signal; exit "$2"; }
trap cleanup 0
trap 'interrupted HUP 129' HUP
trap 'interrupted INT 130' INT
trap 'interrupted TERM 143' TERM
fetch() {
    diagnostic fetch_started
    partial=$(mktemp "$stage/.download.XXXXXXXX")
    case "$source_dir" in
        https://*) curl --fail --silent --location --proto '=https' --proto-redir '=https' --connect-timeout 20 --max-time 300 --max-filesize 536870912 "$source_dir/$1" -o "$partial" 2>/dev/null || fail "Cannot download release asset: $1";;
        *://*) fail 'Remote release sources must use HTTPS';;
        *) cp "$source_dir/$1" "$partial" 2>/dev/null || fail "Cannot copy release asset: $1";;
    esac
    if [ "$1" != SHA256SUMS ]; then verify "$1" "$partial"; fi
    mv "$partial" "$stage/$1" 2>/dev/null || fail "Cannot stage release asset: $1"
    diagnostic fetch_finished
}
verify() {
    expected=$(awk -v name="$1" '$2==name {count++; hash=$1} END {if (count!=1 || length(hash)!=64 || hash !~ /^[0-9a-f]+$/) exit 1; print hash}' "$stage/SHA256SUMS") || fail "Missing or ambiguous checksum: $1"
    if command -v sha256sum >/dev/null 2>&1; then actual=$(sha256sum "$2"); else actual=$(shasum -a 256 "$2"); fi
    actual=${actual%% *}
    [ "$actual" = "$expected" ] || fail "Checksum mismatch: $1"
    diagnostic verified
}
fetch SHA256SUMS
helper="hive-install-$target"
fetch "$helper"
fetch "$helper.LICENSES"
if ! $uninstall; then
    if ! $standalone; then
        part=hive
        $core && part=hive-core
        fetch "$part-$target.tar.gz"
    fi
    if $comb || $standalone; then
        fetch "comb-$target.tar.gz"
    fi
fi
chmod 700 "$stage/$helper"
diagnostic helper_exec
if "$stage/$helper" --staged-from "$stage" "$@"; then
    :
else
    status=$?
    if [ "$status" -eq 126 ]; then
        fail "Cannot execute the installer in $stage. Set TMPDIR to a private directory on an executable filesystem and retry."
    fi
    exit "$status"
fi
