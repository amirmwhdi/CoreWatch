#!/bin/sh
# The Flatpak checks the CI workflow ran: cargo-sources.json lists exactly
# the crates in Cargo.lock, and the Flatpak builds. By default in the image
# CI used (Podman or Docker); --host uses flatpak-builder on this machine.
#
#   tests/flatpak.sh                  check cargo-sources.json, then build
#   tests/flatpak.sh --sources        only the cargo-sources.json check
#   tests/flatpak.sh --fix            regenerate cargo-sources.json from Cargo.lock
#   tests/flatpak.sh --no-sources     only the build
#   tests/flatpak.sh --bundle <file>  also write a .flatpak bundle (inside this folder)
#   tests/flatpak.sh --arch aarch64   build for another architecture (needs QEMU)
#   tests/flatpak.sh --host           use this machine's flatpak-builder
#   tests/flatpak.sh --pull           update the CI image first
#
# The build stays in target/flatpak/. A proxy in HTTPS_PROXY is passed through.
set -eu

cd "$(dirname "$0")/.." || exit 1
repo=$(pwd)

image=ghcr.io/flathub-infra/flatpak-github-actions:gnome-49
manifest=packaging/flatpak/io.github.amirmwhdi.Corewatch.json
app=io.github.amirmwhdi.Corewatch
generator=https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
work=target/flatpak

usage() {
    sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
}

sources=check build=true bundle="" arch="" host=false pull=false inside=false
while [ $# -gt 0 ]; do
    case $1 in
        --sources) build=false ;;
        --fix) sources=fix build=false ;;
        --no-sources) sources=skip ;;
        --bundle) bundle=${2:?--bundle needs a file}; shift ;;
        --arch) arch=${2:?--arch needs x86_64 or aarch64}; shift ;;
        --host) host=true ;;
        --pull) pull=true ;;
        --inside) inside=true ;; # set when this script runs itself in the container
        -h | --help) usage; exit 0 ;;
        *) echo "flatpak.sh: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

if [ -n "$bundle" ]; then
    mkdir -p "$(dirname "$bundle")"
    bundle=$(realpath "$bundle")
    case $bundle in
        "$repo"/*) bundle=${bundle#"$repo"/} ;;
        *) echo "flatpak.sh: the bundle must be inside $repo" >&2; exit 2 ;;
    esac
fi

# The work itself: in the container, or here with --host.
work_here() {
    mkdir -p "$work"
    if [ "$sources" != skip ]; then
        echo "==> cargo-sources.json"
        need python3 "Install Python 3."
        if ! PYTHONPATH="$work/pylib" python3 -c 'import aiohttp, tomlkit' 2>/dev/null; then
            python3 -m pip install --quiet --disable-pip-version-check \
                --target "$work/pylib" aiohttp tomlkit
        fi
        curl -sSfL -o "$work/flatpak-cargo-generator.py" "$generator"
        PYTHONPATH="$work/pylib" python3 "$work/flatpak-cargo-generator.py" \
            Cargo.lock -o "$work/cargo-sources.json"
        if [ "$sources" = fix ]; then
            cp "$work/cargo-sources.json" packaging/flatpak/cargo-sources.json
            echo "packaging/flatpak/cargo-sources.json regenerated"
        elif diff -u packaging/flatpak/cargo-sources.json "$work/cargo-sources.json"; then
            echo "cargo-sources.json matches Cargo.lock"
        else
            echo "cargo-sources.json is out of date: run tests/flatpak.sh --fix" >&2
            return 1
        fi
    fi
    $build || return 0

    echo "==> flatpak build"
    need flatpak-builder "Install flatpak-builder, or leave out --host."
    scope="--user" rofiles=""
    # No FUSE in a container. Its own GNOME runtime is installed system-wide;
    # what else the build needs goes to the user installation, in a volume.
    $inside && rofiles="--disable-rofiles-fuse"

    # shellcheck disable=SC2086 # empty words wanted
    flatpak remote-add $scope --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
    # shellcheck disable=SC2086
    flatpak-builder $scope $rofiles ${arch:+--arch=$arch} --install-deps-from=flathub \
        --force-clean --state-dir="$work/state" --repo="$work/repo" \
        "$work/build" "$manifest"
    if [ -n "$bundle" ]; then
        # shellcheck disable=SC2086
        flatpak build-bundle ${arch:+--arch=$arch} "$work/repo" "$bundle" "$app"
        echo "bundle: $bundle"
    fi
}

need() {
    command -v "$1" >/dev/null 2>&1 && return 0
    echo "flatpak.sh: $1 is not installed. $2" >&2
    exit 1
}

if $inside || $host; then
    status=0
    work_here || status=$?
    # Docker writes as root: give the files back to whoever ran this.
    if $inside && [ -n "${HOST_OWNER:-}" ]; then
        chown -R "$HOST_OWNER" "$work" ${bundle:+"$bundle"} packaging/flatpak/cargo-sources.json
    fi
    exit "$status"
fi

. scripts/lib/containers.sh
cw_engine
if $pull || ! $engine image inspect "$image" >/dev/null 2>&1; then
    $engine pull "$image"
fi
owner=""
if cw_root_owns_files; then owner="$(id -u):$(id -g)"; fi

# The options again, with the bundle's path relative to the repository.
inner="--inside"
case $sources in skip) inner="$inner --no-sources" ;; fix) inner="$inner --fix" ;; esac
$build || [ "$sources" = fix ] || inner="$inner --sources"
inner="$inner${bundle:+ --bundle $bundle}${arch:+ --arch $arch}"

# flatpak-builder needs --privileged for its own sandbox. The image holds
# the GNOME runtime; the Rust extension and the like are kept in a volume,
# so the next run does not download them again.
# shellcheck disable=SC2046,SC2086 # word splitting wanted
exec $engine run --rm --privileged $(cw_proxy_args -e) $cw_git_env \
    -v corewatch-flatpak:/var/lib/corewatch-flatpak \
    -e FLATPAK_USER_DIR=/var/lib/corewatch-flatpak \
    -v "$repo:/src:z" -w /src \
    -e HOST_OWNER="$owner" -e PIP_ROOT_USER_ACTION=ignore \
    "$image" tests/flatpak.sh $inner
