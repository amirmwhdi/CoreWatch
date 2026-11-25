#!/bin/sh
# Build the release packages on this machine, as the Release workflow did
# on GitHub. Every package is built in a container (Podman or Docker) of the
# distribution it is for, from the last commit: uncommitted changes are not
# in the packages.
#
#   release/build.sh                   every package for this machine's architecture
#   release/build.sh deb rpm           only some: deb rpm arch appimage flatpak
#   release/build.sh --arch aarch64    for another architecture, through QEMU
#   release/build.sh --rebuild         rebuild the build images first
#
# The packages and SHA256SUMS land in dist/<version>/; release/publish.sh
# puts them on GitHub. Intermediate files stay in target/packaging/.
# A proxy in HTTPS_PROXY is passed through. See release/README.md.
set -u

cd "$(dirname "$0")/.." || exit 1
repo=$(pwd)
. scripts/lib/containers.sh

usage() {
    sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
}

targets="" arch="" rebuild=false
while [ $# -gt 0 ]; do
    case $1 in
        deb | rpm | arch | appimage | flatpak) targets="$targets $1" ;;
        all) targets="deb rpm arch appimage flatpak" ;;
        --arch) arch=${2:?--arch needs x86_64 or aarch64}; shift ;;
        --rebuild) rebuild=true ;;
        -h | --help) usage; exit 0 ;;
        *) echo "build.sh: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done
[ -n "$targets" ] || targets="deb rpm arch appimage flatpak"

version=$(scripts/version.sh check) || exit 1
host_arch=$(cw_host_arch)
arch=${arch:-$host_arch}
platform=""
if [ "$arch" != "$host_arch" ]; then
    # Another architecture runs under QEMU: install qemu-user-static
    # (binfmt) first. Much slower than a native build.
    platform=$(cw_platform "$arch") || exit 2
fi
case $arch in
    x86_64) debarch=amd64 ;;
    aarch64) debarch=arm64 ;;
    *) echo "build.sh: unsupported architecture $arch" >&2; exit 2 ;;
esac

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "Note: there are uncommitted changes. The packages are built from the last commit ($(git log -1 --format='%h %s')) and do not contain them."
fi

out="dist/$version"
work="target/packaging/$version"
mkdir -p "$out" "$work"

cw_engine
owner=""
if cw_root_owns_files; then owner="$(id -u):$(id -g)"; fi

# step <image> <step script and arguments...>: run release/steps/<step> in
# a fresh container of <image>, as root, with the repository read-only.
step() {
    _image="localhost/corewatch-release-$1-$arch"
    cw_image "$_image" "release/containers/$1.Containerfile" "$rebuild" "$platform" || return 1
    shift
    # shellcheck disable=SC2016,SC2046,SC2086 # the inner script expands in the container
    $engine run --rm --init ${platform:+--platform $platform} $(cw_proxy_args -e) $cw_git_env \
        -v "$repo:/src:ro,z" -w /src \
        -v "$repo/$out:/out:z" -v "$repo/$work:/work:z" \
        -e HOST_OWNER="$owner" \
        "$_image" sh -c '
            status=0
            sh "release/steps/$0" "$@" || status=$?
            [ -z "$HOST_OWNER" ] || chown -R "$HOST_OWNER" /out /work
            exit $status' "$@"
}

summary=""
failed=false
record() { # <target> <status>
    if [ "$2" = 0 ]; then
        summary="$summary
  $(printf '%-9s' "$1") built"
    else
        summary="$summary
  $(printf '%-9s' "$1") FAILED"
        failed=true
    fi
}

source_done=false
need_source() {
    $source_done && return 0
    echo "==> source tarball with vendored crates"
    step debian source.sh "$version" || return 1
    source_done=true
}

for target in $targets; do
    printf '\n\033[1m==> %s (%s)\033[0m\n' "$target" "$arch"
    case $target in
        deb) need_source && step debian deb.sh "$version" "$debarch" ;;
        appimage) need_source && step debian appimage.sh "$version" "$arch" ;;
        rpm) step fedora rpm.sh "$version" "$arch" ;;
        arch)
            if [ "$arch" != x86_64 ]; then
                echo "The Arch package is built for x86_64 only, as before."
                continue
            fi
            step arch arch.sh "$version"
            ;;
        flatpak)
            tests/flatpak.sh --no-sources ${platform:+--arch "$arch"} \
                --bundle "$out/corewatch-$version-$arch.flatpak"
            ;;
    esac
    record "$target" $?
done

# Checksums of everything in the folder, as the release lists them.
(cd "$out" && find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%P\n' |
    sort | xargs -r sha256sum) >"$work/SHA256SUMS" && mv "$work/SHA256SUMS" "$out/SHA256SUMS"

printf '\n\033[1mPackages\033[0m%s\n\nIn %s/:\n' "$summary" "$out"
# shellcheck disable=SC2012 # only shown, our own file names
ls -lh "$out" | tail -n +2
if $failed; then
    exit 1
fi
