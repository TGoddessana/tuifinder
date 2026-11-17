#!/bin/sh
# Download, verify, then atomically install a GitHub Release binary.
set -eu

fail() { printf 'tuifinder: %s\n' "$*" >&2; exit 1; }
fetch() {
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 15 --max-time 300 --retry 2 "$@"
}
valid_version() {
    printf '%s\n' "$1" | LC_ALL=C grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'
}
main() {
    repo=${TUIFINDER_REPO:-TGoddessana/tuifinder}
    version=latest
    install_dir=${HOME:?HOME must be set}/.local/bin
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --repo|--version|--dir)
                [ "$#" -ge 2 ] || fail "Missing value for $1"
                case "$1" in
                    --repo) repo=$2 ;;
                    --version) version=$2 ;;
                    --dir) install_dir=$2 ;;
                esac
                shift 2 ;;
            --help|-h)
                printf '%s\n' 'Usage: sh install.sh [--repo OWNER/REPO] [--version v0.1.0] [--dir PATH]' \
                    'Default: latest release, ~/.local/bin. Re-run to update, or use tuifinder --update.'
                return ;;
            *) fail "Unknown argument: $1" ;;
        esac
    done
    case "$repo" in
        ''|/*|*/|*/*/*|*[!A-Za-z0-9_./-]*|./*|../*|*/.|*/..) fail 'Set TUIFINDER_REPO=OWNER/REPO or pass --repo OWNER/REPO' ;;
        */*) ;;
        *) fail 'Repository must be OWNER/REPO' ;;
    esac
    [ -n "$install_dir" ] || fail 'Install directory cannot be empty'
    case "$version" in latest|v*) ;; *) version=v$version ;; esac
    if [ "$version" != latest ]; then valid_version "$version" || fail 'Version must look like v0.1.0'; fi
    command -v curl >/dev/null 2>&1 || fail 'curl is required'
    if command -v sha256sum >/dev/null 2>&1; then
        checksum=sha256sum
    elif command -v shasum >/dev/null 2>&1; then
        checksum=shasum
    else
        fail 'sha256sum or shasum is required'
    fi
    case "$(uname -s)" in
        Linux) platform=unknown-linux-musl ;;
        Darwin) platform=apple-darwin ;;
        *) fail 'Supported operating systems: Linux and macOS' ;;
    esac
    case "$(uname -m)" in
        x86_64|amd64) arch=x86_64 ;;
        aarch64|arm64) arch=aarch64 ;;
        *) fail 'Supported CPU architectures: x86_64 and ARM64' ;;
    esac
    if [ "$version" = latest ]; then
        resolved=$(fetch --output /dev/null --write-out '%{url_effective}' "https://github.com/$repo/releases/latest") || fail 'Cannot resolve the latest release'
        case "$resolved" in
            "https://github.com/$repo/releases/tag/"*) version=${resolved##*/} ;;
            *) fail 'Unexpected release URL' ;;
        esac
        valid_version "$version" || fail 'Release tag must look like v0.1.0'
    fi
    case "$install_dir" in /*) ;; *) install_dir=$PWD/$install_dir ;; esac
    mkdir -p "$install_dir" || fail "Cannot create $install_dir"
    install_dir=$(CDPATH='' cd -P "$install_dir" && pwd)
    destination=$install_dir/tuifinder
    [ ! -L "$destination" ] || fail 'Destination is a symlink; use the real binary directory with --dir'
    [ ! -d "$destination" ] || fail 'Destination is a directory'
    stage=$(mktemp -d "$install_dir/.tuifinder-install.XXXXXX") || fail 'Cannot create staging directory'
    trap 'rm -rf "$stage"' 0
    trap 'exit 130' INT
    trap 'exit 143' TERM
    trap 'exit 129' HUP
    asset=tuifinder-$arch-$platform
    url=https://github.com/$repo/releases/download/$version
    printf 'Downloading tuifinder %s (%s-%s)…\n' "${version#v}" "$arch" "$platform"
    fetch --output "$stage/tuifinder" "$url/$asset" || fail 'Binary download failed; existing installation preserved'
    fetch --output "$stage/checksum" "$url/$asset.sha256" || fail 'Checksum download failed; existing installation preserved'
    expected=$(awk 'NR == 1 {print $1} END {if (NR != 1) exit 1}' "$stage/checksum") || fail 'Invalid checksum file'
    [ "${#expected}" -eq 64 ] || fail 'Invalid SHA-256 checksum'
    case "$expected" in *[!0-9a-f]*) fail 'Invalid SHA-256 checksum' ;; esac
    if [ "$checksum" = sha256sum ]; then
        actual=$(sha256sum "$stage/tuifinder" | awk '{print $1}')
    else
        actual=$(shasum -a 256 "$stage/tuifinder" | awk '{print $1}')
    fi
    [ "$actual" = "$expected" ] || fail 'Checksum mismatch; existing installation preserved'
    chmod 755 "$stage/tuifinder"
    actual_version=$("$stage/tuifinder" --version) || fail 'Downloaded binary cannot run; existing installation preserved'
    [ "$actual_version" = "tuifinder ${version#v}" ] || fail 'Binary version does not match release tag; existing installation preserved'
    mv -f "$stage/tuifinder" "$destination" || fail 'Cannot replace binary; existing installation preserved'
    printf 'Installed %s to %s\n' "$actual_version" "$destination"
    case ":${PATH:-}:" in
        *":$install_dir:"*) ;;
        *) printf 'Add this directory to PATH: %s\n' "$install_dir" ;;
    esac
}

main "$@"
